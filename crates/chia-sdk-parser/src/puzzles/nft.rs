use chia_protocol::{Bytes32, Coin};
use chia_puzzles::{
    nft::{
        NftOwnershipLayerArgs, NftOwnershipLayerSolution, NftRoyaltyTransferPuzzleArgs,
        NftStateLayerArgs, NftStateLayerSolution, NFT_METADATA_UPDATER_PUZZLE_HASH,
        NFT_OWNERSHIP_LAYER_PUZZLE_HASH, NFT_ROYALTY_TRANSFER_PUZZLE_HASH,
        NFT_STATE_LAYER_PUZZLE_HASH,
    },
    singleton::{SingletonArgs, SingletonStruct},
    Proof,
};
use chia_sdk_types::{
    conditions::{Condition, CreateCoin, NewNftOwner},
    puzzles::NftInfo,
};
use clvm_traits::FromClvm;
use clvm_utils::tree_hash;
use clvmr::{Allocator, NodePtr};

use crate::{puzzle_conditions, ParseError, Puzzle, SingletonPuzzle};

#[derive(Debug, Clone, Copy)]
pub struct NftPuzzle {
    pub p2_puzzle: Puzzle,
    pub metadata: NodePtr,
    pub current_owner: Option<Bytes32>,
    pub royalty_puzzle_hash: Bytes32,
    pub royalty_percentage: u16,
}

impl NftPuzzle {
    pub fn parse(
        allocator: &Allocator,
        launcher_id: Bytes32,
        puzzle: &Puzzle,
    ) -> Result<Option<Self>, ParseError> {
        let Some(puzzle) = puzzle.as_curried() else {
            return Ok(None);
        };

        if puzzle.mod_hash != NFT_STATE_LAYER_PUZZLE_HASH {
            return Ok(None);
        }

        let state_args = NftStateLayerArgs::<NodePtr, NodePtr>::from_clvm(allocator, puzzle.args)?;

        if state_args.mod_hash != NFT_STATE_LAYER_PUZZLE_HASH.into() {
            return Err(ParseError::InvalidModHash);
        }

        if state_args.metadata_updater_puzzle_hash != NFT_METADATA_UPDATER_PUZZLE_HASH.into() {
            return Err(ParseError::NonStandardLayer);
        }

        let Some(inner_puzzle) = Puzzle::parse(allocator, state_args.inner_puzzle).as_curried()
        else {
            return Err(ParseError::NonStandardLayer);
        };

        let ownership_args =
            NftOwnershipLayerArgs::<NodePtr, NodePtr>::from_clvm(allocator, inner_puzzle.args)?;

        if ownership_args.mod_hash != NFT_OWNERSHIP_LAYER_PUZZLE_HASH.into() {
            return Err(ParseError::NonStandardLayer);
        }

        let Some(transfer_puzzle) =
            Puzzle::parse(allocator, ownership_args.transfer_program).as_curried()
        else {
            return Err(ParseError::NonStandardLayer);
        };

        if transfer_puzzle.mod_hash != NFT_ROYALTY_TRANSFER_PUZZLE_HASH {
            return Err(ParseError::NonStandardLayer);
        }

        let transfer_args =
            NftRoyaltyTransferPuzzleArgs::from_clvm(allocator, transfer_puzzle.args)?;

        if transfer_args.singleton_struct != SingletonStruct::new(launcher_id) {
            return Err(ParseError::InvalidSingletonStruct);
        }

        Ok(Some(Self {
            p2_puzzle: Puzzle::parse(allocator, ownership_args.inner_puzzle),
            metadata: state_args.metadata,
            current_owner: ownership_args.current_owner,
            royalty_puzzle_hash: transfer_args.royalty_puzzle_hash,
            royalty_percentage: transfer_args.trade_price_percentage,
        }))
    }

    pub fn output(
        &self,
        allocator: &mut Allocator,
        solution: NodePtr,
    ) -> Result<(Option<CreateCoin>, Option<NewNftOwner>), ParseError> {
        let state_solution = NftStateLayerSolution::from_clvm(allocator, solution)?;
        let ownership_solution =
            NftOwnershipLayerSolution::from_clvm(allocator, state_solution.inner_solution)?;

        let conditions = puzzle_conditions(
            allocator,
            self.p2_puzzle.ptr(),
            ownership_solution.inner_solution,
        )?;

        let create_coin = conditions.iter().find_map(|condition| match condition {
            Condition::CreateCoin(create_coin) => Some(create_coin),
            _ => None,
        });

        let new_owner = conditions.iter().find_map(|condition| match condition {
            Condition::NewNftOwner(new_owner) => Some(new_owner),
            _ => None,
        });

        Ok((create_coin.cloned(), new_owner.cloned()))
    }

    pub fn child_coin_info(
        &self,
        allocator: &mut Allocator,
        singleton: &SingletonPuzzle,
        parent_coin: Coin,
        child_coin: Coin,
        solution: NodePtr,
    ) -> Result<NftInfo<NodePtr>, ParseError> {
        let (create_coin, new_owner) = self.output(allocator, solution)?;
        let create_coin = create_coin.ok_or(ParseError::MissingChild)?;

        let current_owner = if let Some(condition) = new_owner {
            condition.new_owner
        } else {
            self.current_owner
        };

        let transfer = NftRoyaltyTransferPuzzleArgs::curry_tree_hash(
            singleton.launcher_id,
            self.royalty_puzzle_hash,
            self.royalty_percentage,
        );

        let ownership = NftOwnershipLayerArgs::curry_tree_hash(
            current_owner,
            transfer,
            create_coin.puzzle_hash.into(),
        );

        let state =
            NftStateLayerArgs::curry_tree_hash(tree_hash(allocator, self.metadata), ownership);

        let singleton_puzzle_hash = SingletonArgs::curry_tree_hash(singleton.launcher_id, state);

        if singleton_puzzle_hash != child_coin.puzzle_hash.into() {
            return Err(ParseError::MismatchedOutput);
        }

        Ok(NftInfo {
            launcher_id: singleton.launcher_id,
            coin: child_coin,
            p2_puzzle_hash: create_coin.puzzle_hash,
            nft_inner_puzzle_hash: state.into(),
            royalty_percentage: self.royalty_percentage,
            royalty_puzzle_hash: self.royalty_puzzle_hash,
            current_owner,
            metadata: self.metadata,
            proof: Proof::Lineage(singleton.lineage_proof(parent_coin)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chia_bls::PublicKey;
    use chia_protocol::{Bytes32, Coin};
    use chia_puzzles::{singleton::SingletonSolution, standard::StandardArgs};
    use chia_sdk_driver::{
        CreateDid, Launcher, MintNft, OwnerDid, SpendContext, StandardMint, StandardSpend,
    };
    use clvm_traits::ToNodePtr;

    #[test]
    fn test_parse_nft() -> anyhow::Result<()> {
        let mut allocator = Allocator::new();
        let ctx = &mut SpendContext::new(&mut allocator);

        let pk = PublicKey::default();
        let puzzle_hash = StandardArgs::curry_tree_hash(pk).into();
        let parent = Coin::new(Bytes32::default(), puzzle_hash, 2);

        let (create_did, did_info) = Launcher::new(parent.coin_id(), 1)
            .create(ctx)?
            .create_standard_did(ctx, pk)?;

        let (mint_nft, nft_info) = Launcher::new(did_info.coin.coin_id(), 1)
            .create(ctx)?
            .mint_standard_nft(
                ctx,
                StandardMint {
                    metadata: (),
                    royalty_percentage: 300,
                    royalty_puzzle_hash: Bytes32::new([1; 32]),
                    owner_puzzle_hash: puzzle_hash,
                    synthetic_key: pk,
                    owner_did: Some(OwnerDid {
                        did_id: did_info.launcher_id,
                        did_inner_puzzle_hash: did_info.did_inner_puzzle_hash,
                    }),
                },
            )?;

        StandardSpend::new()
            .chain(create_did)
            .chain(mint_nft)
            .finish(ctx, parent, pk)?;

        let coin_spends = ctx.take_spends();

        let coin_spend = coin_spends
            .into_iter()
            .find(|cs| cs.coin.coin_id() == nft_info.coin.parent_coin_info)
            .unwrap();

        let puzzle_ptr = coin_spend.puzzle_reveal.to_node_ptr(&mut allocator)?;
        let solution_ptr = coin_spend.solution.to_node_ptr(&mut allocator)?;

        let puzzle = Puzzle::parse(&allocator, puzzle_ptr);

        let singleton =
            SingletonPuzzle::parse(&allocator, &puzzle)?.expect("not a singleton puzzle");
        let singleton_solution = SingletonSolution::<NodePtr>::from_clvm(&allocator, solution_ptr)?;

        let nft = NftPuzzle::parse(&allocator, singleton.launcher_id, &singleton.inner_puzzle)?
            .expect("not an nft puzzle");

        let parsed_nft_info = nft.child_coin_info(
            &mut allocator,
            &singleton,
            coin_spend.coin,
            nft_info.coin,
            singleton_solution.inner_solution,
        )?;

        assert_eq!(parsed_nft_info, nft_info.with_metadata(NodePtr::NIL));

        Ok(())
    }
}