use chia_protocol::{Bytes32, Coin};
use chia_puzzles::singleton::{SINGLETON_LAUNCHER_PUZZLE_HASH, SINGLETON_TOP_LAYER_PUZZLE_HASH};
use clvm_traits::{FromClvm, ToClvm};
use clvm_utils::{CurriedProgram, ToTreeHash, TreeHash};
use clvmr::{Allocator, NodePtr};
use hex_literal::hex;

use crate::{DriverError, Layer, Puzzle, Spend, SpendContext};

/// The p2 delegated singleton [`Layer`] allows for requiring that a singleton
/// be spent alongside this coin to authorize it, while also outputting conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2DelegatedSingletonLayer {
    pub launcher_id: Bytes32,
}

impl P2DelegatedSingletonLayer {
    pub fn new(launcher_id: Bytes32) -> Self {
        Self { launcher_id }
    }

    pub fn spend(
        &self,
        ctx: &mut SpendContext,
        coin_id: Bytes32,
        singleton_inner_puzzle_hash: Bytes32,
        delegated_spend: Spend,
    ) -> Result<Spend, DriverError> {
        let puzzle = self.construct_puzzle(ctx)?;
        let solution = self.construct_solution(
            ctx,
            P2DelegatedSingletonSolution {
                singleton_inner_puzzle_hash,
                coin_id,
                delegated_puzzle: delegated_spend.puzzle,
                delegated_solution: delegated_spend.solution,
            },
        )?;
        Ok(Spend { puzzle, solution })
    }

    pub fn spend_coin(
        &self,
        ctx: &mut SpendContext,
        coin: Coin,
        singleton_inner_puzzle_hash: Bytes32,
        delegated_spend: Spend,
    ) -> Result<(), DriverError> {
        let coin_spend = self.construct_coin_spend(
            ctx,
            coin,
            P2DelegatedSingletonSolution {
                singleton_inner_puzzle_hash,
                coin_id: coin.coin_id(),
                delegated_puzzle: delegated_spend.puzzle,
                delegated_solution: delegated_spend.solution,
            },
        )?;
        ctx.insert(coin_spend);
        Ok(())
    }
}

impl Layer for P2DelegatedSingletonLayer {
    type Solution = P2DelegatedSingletonSolution<NodePtr, NodePtr>;

    fn parse_puzzle(allocator: &Allocator, puzzle: Puzzle) -> Result<Option<Self>, DriverError> {
        let Some(puzzle) = puzzle.as_curried() else {
            return Ok(None);
        };

        if puzzle.mod_hash != P2_DELEGATED_SINGLETON_PUZZLE_HASH {
            return Ok(None);
        }

        let args = P2DelegatedSingletonArgs::from_clvm(allocator, puzzle.args)?;

        if args.singleton_mod_hash != SINGLETON_TOP_LAYER_PUZZLE_HASH.into()
            || args.launcher_puzzle_hash != SINGLETON_LAUNCHER_PUZZLE_HASH.into()
        {
            return Err(DriverError::InvalidSingletonStruct);
        }

        Ok(Some(Self {
            launcher_id: args.launcher_id,
        }))
    }

    fn parse_solution(
        allocator: &Allocator,
        solution: NodePtr,
    ) -> Result<Self::Solution, DriverError> {
        Ok(P2DelegatedSingletonSolution::from_clvm(
            allocator, solution,
        )?)
    }

    fn construct_puzzle(&self, ctx: &mut SpendContext) -> Result<NodePtr, DriverError> {
        let curried = CurriedProgram {
            program: ctx.p2_delegated_singleton_puzzle()?,
            args: P2DelegatedSingletonArgs::new(self.launcher_id),
        };
        ctx.alloc(&curried)
    }

    fn construct_solution(
        &self,
        ctx: &mut SpendContext,
        solution: Self::Solution,
    ) -> Result<NodePtr, DriverError> {
        ctx.alloc(&solution)
    }
}

impl ToTreeHash for P2DelegatedSingletonLayer {
    fn tree_hash(&self) -> TreeHash {
        P2DelegatedSingletonArgs::curry_tree_hash(self.launcher_id)
    }
}

// (mod (SINGLETON_MOD_HASH LAUNCHER_ID LAUNCHER_PUZZLE_HASH singleton_inner_puzzle_hash delegated_puzzle delegated_solution my_id)

#[derive(Debug, Clone, Copy, PartialEq, Eq, ToClvm, FromClvm)]
#[clvm(curry)]
pub struct P2DelegatedSingletonArgs {
    pub singleton_mod_hash: Bytes32,
    pub launcher_id: Bytes32,
    pub launcher_puzzle_hash: Bytes32,
}

impl P2DelegatedSingletonArgs {
    pub fn new(launcher_id: Bytes32) -> Self {
        Self {
            singleton_mod_hash: SINGLETON_TOP_LAYER_PUZZLE_HASH.into(),
            launcher_id,
            launcher_puzzle_hash: SINGLETON_LAUNCHER_PUZZLE_HASH.into(),
        }
    }

    pub fn curry_tree_hash(launcher_id: Bytes32) -> TreeHash {
        CurriedProgram {
            program: P2_DELEGATED_SINGLETON_PUZZLE_HASH,
            args: Self::new(launcher_id),
        }
        .tree_hash()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ToClvm, FromClvm)]
#[clvm(list)]
pub struct P2DelegatedSingletonSolution<P, S> {
    pub singleton_inner_puzzle_hash: Bytes32,
    pub delegated_puzzle: P,
    pub delegated_solution: S,
    pub coin_id: Bytes32,
}

pub const P2_DELEGATED_SINGLETON_PUZZLE: [u8; 508] = hex!(
    "
    ff02ffff01ff02ff16ffff04ff02ffff04ffff04ffff04ff28ffff04ffff0bff
    ff02ff2effff04ff02ffff04ff05ffff04ff2fffff04ffff02ff3effff04ff02
    ffff04ffff04ff05ffff04ff0bff178080ff80808080ff808080808080ff8201
    7f80ff808080ffff04ffff04ff14ffff04ffff02ff3effff04ff02ffff04ff5f
    ff80808080ff808080ffff04ffff04ff10ffff04ff82017fff808080ff808080
    80ffff04ffff02ff5fff81bf80ff8080808080ffff04ffff01ffffff46ff3f02
    ff3cff0401ffff01ff02ff02ffff03ff05ffff01ff02ff3affff04ff02ffff04
    ff0dffff04ffff0bff2affff0bff3cff2c80ffff0bff2affff0bff2affff0bff
    3cff1280ff0980ffff0bff2aff0bffff0bff3cff8080808080ff8080808080ff
    ff010b80ff0180ffff02ffff03ff05ffff01ff04ff09ffff02ff16ffff04ff02
    ffff04ff0dffff04ff0bff808080808080ffff010b80ff0180ffff0bff2affff
    0bff3cff3880ffff0bff2affff0bff2affff0bff3cff1280ff0580ffff0bff2a
    ffff02ff3affff04ff02ffff04ff07ffff04ffff0bff3cff3c80ff8080808080
    ffff0bff3cff8080808080ff02ffff03ffff07ff0580ffff01ff0bffff0102ff
    ff02ff3effff04ff02ffff04ff09ff80808080ffff02ff3effff04ff02ffff04
    ff0dff8080808080ffff01ff0bffff0101ff058080ff0180ff018080
    "
);

pub const P2_DELEGATED_SINGLETON_PUZZLE_HASH: TreeHash = TreeHash::new(hex!(
    "2cadfbf73f1ff120d708ad2fefad1c78eefb8d874231bc87eac7c2df5eeb904a"
));

#[cfg(test)]
mod tests {
    use super::*;

    use crate::assert_puzzle_hash;

    #[test]
    fn test_puzzle_hash() -> anyhow::Result<()> {
        assert_puzzle_hash!(P2_DELEGATED_SINGLETON_PUZZLE => P2_DELEGATED_SINGLETON_PUZZLE_HASH);
        Ok(())
    }
}