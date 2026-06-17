use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;

use super::swap_v2::{exact_internal_v2_with_fee_rate, swap_v2, SwapSingleV2};

pub fn swap_v3_dyn<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, SwapSingleV2<'info>>,
    amount: u64,
    other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
    is_base_input: bool,
) -> Result<()> {
    let mut pool_state = ctx.accounts.pool_state.load_mut()?;
    if !pool_state.is_swap_dynamic_fee_enabled() {
        drop(pool_state);
        return swap_v2(ctx, amount, other_amount_threshold, sqrt_price_limit_x64, is_base_input);
    }

    let remaining_len = ctx.remaining_accounts.len();
    require!(remaining_len >= 2, ErrorCode::AccountLack);

    let token0_pyth_oracle = &ctx.remaining_accounts[remaining_len - 2];
    let token1_pyth_oracle = &ctx.remaining_accounts[remaining_len - 1];

    let zero_for_one = ctx.accounts.input_vault.mint == pool_state.token_mint_0;
    let block_timestamp = oracle::block_timestamp() as u64;
    let fee_base = pool_state.calculate_base_trade_fee_rate(&ctx.accounts.amm_config, zero_for_one, block_timestamp)?;
    let decay_trade_fee_rate = pool_state.get_decay_trade_fee_rate_with_swap_side(zero_for_one, block_timestamp);
    let effective_base_rate = pool_state.get_effective_trade_fee_rate(&ctx.accounts.amm_config);
    pool_state.disable_decay_fee_if_needed(zero_for_one, decay_trade_fee_rate, effective_base_rate)?;

    let fee_result = pool_state.calculate_dynamic_fee_rate(
        ctx.accounts.input_vault.key(),
        ctx.accounts.input_vault.mint,
        ctx.accounts.input_vault.amount,
        ctx.accounts.output_vault.amount,
        amount,
        is_base_input,
        other_amount_threshold,
        fee_base,
        token0_pyth_oracle,
        token1_pyth_oracle,
    )?;
    let total_fee_rate = fee_result.total_fee_rate;

    drop(pool_state);

    let amount_result = exact_internal_v2_with_fee_rate(
        ctx.accounts,
        &ctx.remaining_accounts[..remaining_len - 2],
        amount,
        sqrt_price_limit_x64,
        is_base_input,
        total_fee_rate,
    )?;

    if is_base_input {
        require_gte!(
            amount_result,
            other_amount_threshold,
            ErrorCode::TooLittleOutputReceived
        );
    } else {
        require_gte!(other_amount_threshold, amount_result, ErrorCode::TooMuchInputPaid);
    }

    Ok(())
}
