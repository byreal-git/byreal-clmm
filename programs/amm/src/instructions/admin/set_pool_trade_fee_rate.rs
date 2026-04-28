use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct SetPoolTradeFeeRate<'info> {
    /// Must be the pool_manager authority (matches create_pool)
    #[account(address = admin_group.pool_manager @ ErrorCode::NotApproved)]
    pub authority: Signer<'info>,

    #[account(
        seeds = [ADMIN_GROUP_SEED.as_bytes()],
        bump,
    )]
    pub admin_group: Box<Account<'info, AmmAdminGroup>>,

    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,
}

/// Set a pool-specific trade_fee_rate.
/// If `trade_fee_rate` == 0, the trade_fee_rate from amm_config is used.
/// If `trade_fee_rate` > 0, this value is used as the pool's base fee rate.
pub fn set_pool_trade_fee_rate(ctx: Context<SetPoolTradeFeeRate>, trade_fee_rate: u32) -> Result<()> {
    let mut pool_state = ctx.accounts.pool_state.load_mut()?;
    pool_state.trade_fee_rate = trade_fee_rate;
    Ok(())
}
