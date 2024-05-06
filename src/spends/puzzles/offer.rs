mod offer_builder;
mod offer_compression;
mod offer_encoding;
mod settlement_payments;

pub use offer_builder::*;
pub use offer_compression::*;
pub use offer_encoding::*;
pub use settlement_payments::*;

use chia_protocol::{CoinSpend, SpendBundle};

#[derive(Debug, Clone)]
pub struct Offer {
    offered_spend_bundle: SpendBundle,
    requested_payment_spends: Vec<CoinSpend>,
}

impl From<SpendBundle> for Offer {
    fn from(spend_bundle: SpendBundle) -> Self {
        let (requested_payment_spends, coin_spends): (_, Vec<_>) = spend_bundle
            .coin_spends
            .into_iter()
            .partition(|coin_spend| {
                coin_spend
                    .coin
                    .parent_coin_info
                    .iter()
                    .all(|byte| *byte == 0)
            });

        let offered_spend_bundle = SpendBundle::new(coin_spends, spend_bundle.aggregated_signature);

        Self {
            offered_spend_bundle,
            requested_payment_spends,
        }
    }
}

impl From<Offer> for SpendBundle {
    fn from(offer: Offer) -> Self {
        let mut spend_bundle = offer.offered_spend_bundle;

        spend_bundle
            .coin_spends
            .extend(offer.requested_payment_spends);

        spend_bundle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chia_bls::{sign, DerivableKey, SecretKey, Signature};
    use chia_protocol::{Bytes32, Coin, SpendBundle};
    use chia_wallet::{
        cat::{cat_puzzle_hash, CatArgs, CAT_PUZZLE_HASH},
        offer::SETTLEMENT_PAYMENTS_PUZZLE_HASH,
        standard::{standard_puzzle_hash, DEFAULT_HIDDEN_PUZZLE_HASH},
        DeriveSynthetic, LineageProof,
    };
    use clvm_utils::CurriedProgram;
    use clvmr::Allocator;

    use crate::{
        testing::SECRET_KEY, AssertPuzzleAnnouncement, CatSpend, Chainable, CreateCoinWithMemos,
        CreateCoinWithoutMemos, InnerSpend, IssueCat, RequiredSignature, SpendContext,
        StandardSpend, WalletSimulator,
    };

    fn sk1() -> SecretKey {
        SECRET_KEY
            .derive_unhardened(0)
            .derive_synthetic(&DEFAULT_HIDDEN_PUZZLE_HASH)
    }

    fn sk2() -> SecretKey {
        SECRET_KEY
            .derive_unhardened(1)
            .derive_synthetic(&DEFAULT_HIDDEN_PUZZLE_HASH)
    }

    fn sign_tx(required_signatures: Vec<RequiredSignature>) -> Signature {
        let sk1 = sk1();
        let sk2 = sk2();

        let pk1 = sk1.public_key();
        let pk2 = sk2.public_key();

        let mut aggregated_signature = Signature::default();

        for req in required_signatures {
            if req.public_key() == &pk1 {
                let sig = sign(&sk1, &req.final_message());
                aggregated_signature += &sig;
            } else if req.public_key() == &pk2 {
                let sig = sign(&sk2, &req.final_message());
                aggregated_signature += &sig;
            } else {
                panic!("unexpected public key");
            }
        }

        aggregated_signature
    }

    #[tokio::test]
    async fn test_offer_bundle() -> anyhow::Result<()> {
        let sim = WalletSimulator::new().await;
        let peer = sim.peer().await;

        let mut allocator = Allocator::new();
        let mut ctx = SpendContext::new(&mut allocator);

        let sk = sk1();
        let pk = sk.public_key();

        let puzzle_hash = Bytes32::new(standard_puzzle_hash(&pk));

        let parent = sim.generate_coin(puzzle_hash, 1000).await.coin;

        let (issue_cat, cat_info) = IssueCat::new(parent.coin_id())
            .condition(ctx.alloc(CreateCoinWithMemos {
                puzzle_hash,
                amount: 1000,
                memos: vec![puzzle_hash.to_vec().into()],
            })?)
            .multi_issuance(&mut ctx, pk.clone(), 1000)?;

        let coin_spends =
            StandardSpend::new()
                .chain(issue_cat)
                .finish(&mut ctx, parent.clone(), pk.clone())?;

        let mut spend_bundle = SpendBundle::new(coin_spends, Signature::default());

        let required_signatures = RequiredSignature::from_coin_spends(
            ctx.allocator_mut(),
            &spend_bundle.coin_spends,
            WalletSimulator::AGG_SIG_ME.into(),
        )?;

        spend_bundle.aggregated_signature = sign_tx(required_signatures);

        let ack = peer.send_transaction(spend_bundle).await?;
        assert_eq!(ack.error, None);
        assert_eq!(ack.status, 1);

        // Prepare offer contents.
        let cat = Coin::new(
            cat_info.eve_coin.coin_id(),
            cat_puzzle_hash(cat_info.asset_id.into(), puzzle_hash.into()).into(),
            1000,
        );

        let xch = sim.generate_coin(puzzle_hash, 1000).await.coin;

        let xch_payment = NotarizedPayment {
            nonce: calculate_nonce(vec![cat.coin_id()]),
            payments: vec![Payment::WithoutMemos(PaymentWithoutMemos {
                puzzle_hash,
                amount: 1000,
            })],
        };

        let cat_payment = NotarizedPayment {
            nonce: calculate_nonce(vec![xch.coin_id()]),
            payments: vec![Payment::WithoutMemos(PaymentWithoutMemos {
                puzzle_hash,
                amount: 1000,
            })],
        };

        let cat_puzzle = ctx.cat_puzzle();
        let settlement_payments_puzzle = ctx.settlement_payments_puzzle();

        let cat_settlements = ctx.alloc(CurriedProgram {
            program: cat_puzzle,
            args: CatArgs {
                mod_hash: CAT_PUZZLE_HASH.into(),
                tail_program_hash: cat_info.asset_id,
                inner_puzzle: settlement_payments_puzzle,
            },
        })?;

        let cat_settlements_hash = ctx.tree_hash(cat_settlements);

        let assert_xch = offer_announcement_id(
            &mut ctx,
            SETTLEMENT_PAYMENTS_PUZZLE_HASH.into(),
            xch_payment.clone(),
        )?;

        let assert_cat =
            offer_announcement_id(&mut ctx, cat_settlements_hash, cat_payment.clone())?;

        let mut coin_spends = Vec::new();

        let lineage_proof = LineageProof {
            parent_coin_info: parent.coin_id(),
            inner_puzzle_hash: cat_info.eve_inner_puzzle_hash,
            amount: 1000,
        };

        let (inner_spend, _) = StandardSpend::new()
            .condition(ctx.alloc(CreateCoinWithMemos {
                puzzle_hash: SETTLEMENT_PAYMENTS_PUZZLE_HASH.into(),
                amount: 1000,
                memos: vec![SETTLEMENT_PAYMENTS_PUZZLE_HASH.to_vec().into()],
            })?)
            .condition(ctx.alloc(AssertPuzzleAnnouncement {
                announcement_id: assert_xch,
            })?)
            .inner_spend(&mut ctx, pk.clone())?;

        let cat_to_settlement = CatSpend::new(cat_info.asset_id)
            .spend(cat.clone(), inner_spend, lineage_proof, 0)
            .finish(&mut ctx)?;

        coin_spends.extend(cat_to_settlement);

        let cat_settlement_coin = Coin::new(
            cat.coin_id(),
            cat_puzzle_hash(cat_info.asset_id.into(), SETTLEMENT_PAYMENTS_PUZZLE_HASH).into(),
            1000,
        );

        let xch_to_settlement = StandardSpend::new()
            .condition(ctx.alloc(CreateCoinWithoutMemos {
                puzzle_hash: SETTLEMENT_PAYMENTS_PUZZLE_HASH.into(),
                amount: 1000,
            })?)
            .condition(ctx.alloc(AssertPuzzleAnnouncement {
                announcement_id: assert_cat,
            })?)
            .finish(&mut ctx, xch.clone(), pk)?;

        coin_spends.extend(xch_to_settlement);

        let xch_settlement_coin =
            Coin::new(xch.coin_id(), SETTLEMENT_PAYMENTS_PUZZLE_HASH.into(), 1000);

        let lineage_proof = LineageProof {
            parent_coin_info: cat_info.eve_coin.coin_id(),
            inner_puzzle_hash: puzzle_hash,
            amount: 1000,
        };

        let solution = ctx.alloc(SettlementPaymentsSolution {
            notarized_payments: vec![cat_payment],
        })?;
        let inner_spend = InnerSpend::new(settlement_payments_puzzle, solution);

        let settlement_to_cat = CatSpend::new(cat_info.asset_id)
            .spend(cat_settlement_coin, inner_spend, lineage_proof, 0)
            .finish(&mut ctx)?;

        coin_spends.extend(settlement_to_cat);

        let puzzle_reveal = ctx.serialize(settlement_payments_puzzle)?;
        let solution = ctx.serialize(SettlementPaymentsSolution {
            notarized_payments: vec![xch_payment],
        })?;

        let settlement_to_xch = CoinSpend::new(xch_settlement_coin, puzzle_reveal, solution);

        coin_spends.push(settlement_to_xch);

        let mut spend_bundle = SpendBundle::new(coin_spends, Signature::default());

        let required_signatures = RequiredSignature::from_coin_spends(
            ctx.allocator_mut(),
            &spend_bundle.coin_spends,
            WalletSimulator::AGG_SIG_ME.into(),
        )?;

        spend_bundle.aggregated_signature = sign_tx(required_signatures);

        let ack = peer.send_transaction(spend_bundle).await?;
        assert_eq!(ack.error, None);
        assert_eq!(ack.status, 1);

        Ok(())
    }
}
