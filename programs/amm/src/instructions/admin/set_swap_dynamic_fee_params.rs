use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SetSwapDynamicFeeParamsInput {
    /// Whether to enable swap-dynamic-fee
    pub enabled: bool,
    /// arbitrage_fee buffer value, in ppm (10^-6)
    pub arbitrage_fee_buffer_ppm: Option<u16>,
    /// trade_slippage_fee base, precision 0.001 bps
    pub trade_slippage_fee_base_milli_bp: Option<u8>,
    /// trade_slippage_fee threshold, in units of 100
    pub trade_slippage_fee_trade_size_threshold: Option<u8>,
    /// imbalance_fee base, precision 0.1 bps
    pub imbalance_fee_base_tenths_of_bp: Option<u8>,
    /// imbalance_fee threshold, in units of 1/100
    pub imbalance_fee_x: Option<u8>,
    /// Pyth feed id for token0
    pub token0_pyth_feed_id: Option<[u8; 32]>,
    /// Pyth feed id for token1
    pub token1_pyth_feed_id: Option<[u8; 32]>,
}

#[derive(Accounts)]
pub struct SetSwapDynamicFeeParams<'info> {
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

pub fn set_swap_dynamic_fee_params(
    ctx: Context<SetSwapDynamicFeeParams>,
    params: SetSwapDynamicFeeParamsInput,
) -> Result<()> {
    let mut pool_state = ctx.accounts.pool_state.load_mut()?;

    pool_state.set_swap_dynamic_fee_enabled(params.enabled);

    pool_state.set_swap_dynamic_fee_params(
        params.arbitrage_fee_buffer_ppm,
        params.trade_slippage_fee_base_milli_bp,
        params.trade_slippage_fee_trade_size_threshold,
        params.imbalance_fee_base_tenths_of_bp,
        params.imbalance_fee_x,
        params.token0_pyth_feed_id,
        params.token1_pyth_feed_id,
    )?;

    Ok(())
}
