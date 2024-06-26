use chia_protocol::CoinSpend;
use chia_puzzles::{
    did::{DidArgs, DidSolution},
    singleton::SingletonStruct,
};
use chia_sdk_types::puzzles::DidInfo;
use clvm_traits::ToClvm;
use clvm_utils::CurriedProgram;
use clvmr::NodePtr;

use crate::{puzzles::spend_singleton, Spend, SpendContext, SpendError};

pub fn did_spend<M>(
    ctx: &mut SpendContext,
    did_info: &DidInfo<M>,
    inner_spend: Spend,
) -> Result<CoinSpend, SpendError>
where
    M: ToClvm<NodePtr>,
{
    let did_inner_puzzle = ctx.did_inner_puzzle()?;

    let puzzle = ctx.alloc(&CurriedProgram {
        program: did_inner_puzzle,
        args: DidArgs {
            inner_puzzle: inner_spend.puzzle(),
            recovery_did_list_hash: did_info.recovery_did_list_hash,
            num_verifications_required: did_info.num_verifications_required,
            singleton_struct: SingletonStruct::new(did_info.launcher_id),
            metadata: &did_info.metadata,
        },
    })?;

    let solution = ctx.alloc(&DidSolution::InnerSpend(inner_spend.solution()))?;

    spend_singleton(
        ctx,
        did_info.coin,
        did_info.launcher_id,
        did_info.proof,
        Spend::new(puzzle, solution),
    )
}

#[cfg(test)]
mod tests {
    use chia_puzzles::standard::StandardArgs;
    use chia_sdk_test::{secret_key, test_transaction, Simulator};

    use crate::{Conditions, Launcher};

    use super::*;

    #[tokio::test]
    async fn test_did_recreation() -> anyhow::Result<()> {
        let sim = Simulator::new().await?;
        let peer = sim.connect().await?;
        let ctx = &mut SpendContext::new();

        let sk = secret_key()?;
        let pk = sk.public_key();

        let puzzle_hash = StandardArgs::curry_tree_hash(pk).into();
        let coin = sim.mint_coin(puzzle_hash, 1).await;

        let (create_did, mut did_info) =
            Launcher::new(coin.coin_id(), 1).create_simple_did(ctx, pk)?;

        ctx.spend_p2_coin(coin, pk, create_did)?;

        for _ in 0..10 {
            did_info = ctx.spend_standard_did(did_info, pk, Conditions::new())?;
        }

        test_transaction(
            &peer,
            ctx.take_spends(),
            &[sk],
            sim.config().genesis_challenge,
        )
        .await;

        let coin_state = sim
            .coin_state(did_info.coin.coin_id())
            .await
            .expect("expected did coin");
        assert_eq!(coin_state.coin, did_info.coin);

        Ok(())
    }
}
