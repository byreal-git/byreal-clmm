use std::collections::VecDeque;
use std::ops::Deref;

use crate::error::ErrorCode;
use crate::libraries::tick_math;
use crate::swap::swap_internal_with_fee_rate;
use crate::util::*;
use crate::{states::*, util};
use anchor_lang::{prelude::*, solana_program};
use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};

/// Memo msg for swap
pub const SWAP_MEMO_MSG: &'static [u8] = b"byreal_swap";
#[derive(Accounts)]
pub struct SwapSingleV2<'info> {
    /// The user performing the swap
    pub payer: Signer<'info>,

    /// The factory state to read protocol fees
    #[account(address = pool_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// The program account of the pool in which the swap will be performed
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// The user token account for input token
    #[account(mut)]
    pub input_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The user token account for output token
    #[account(mut)]
    pub output_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for input token
    #[account(mut)]
    pub input_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for output token
    #[account(mut)]
    pub output_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The program account for the most recent oracle observation
    #[account(mut, address = pool_state.load()?.observation_key)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    /// SPL program for token transfers
    pub token_program: Program<'info, Token>,

    /// SPL program 2022 for token transfers
    pub token_program_2022: Program<'info, Token2022>,

    /// Memo program
    pub memo_program: Program<'info, Memo>,

    /// The mint of token vault 0
    #[account(
        address = input_vault.mint
    )]
    pub input_vault_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint of token vault 1
    #[account(
        address = output_vault.mint
    )]
    pub output_vault_mint: Box<InterfaceAccount<'info, Mint>>,
    // remaining accounts
    // tickarray_bitmap_extension: must add account if need regardless the sequence
    // tick_array_account_1
    // tick_array_account_2
    // tick_array_account_...
}

/// Performs a single exact input/output swap
/// if is_base_input = true, return value is the max_amount_out, otherwise is min_amount_in
pub fn exact_internal_v2<'c: 'info, 'info>(
    ctx: &mut SwapSingleV2<'info>,
    remaining_accounts: &'c [AccountInfo<'info>],
    amount_specified: u64,
    sqrt_price_limit_x64: u128,
    is_base_input: bool,
) -> Result<u64> {
    let trade_fee_rate = {
        let mut pool_state = ctx.pool_state.load_mut()?;
        let zero_for_one = ctx.input_vault.mint == pool_state.token_mint_0;
        let block_timestamp = oracle::block_timestamp() as u64;
        let trade_fee_rate =
            pool_state.calculate_base_trade_fee_rate(&ctx.amm_config, zero_for_one, block_timestamp)?;
        let decay_trade_fee_rate = pool_state.get_decay_trade_fee_rate_with_swap_side(zero_for_one, block_timestamp);
        // Use decay_trade_fee_rate and effective base_trade_fee_rate to decide whether to disable decay fee:
        // if decay_trade_fee_rate has already converged to base_trade_fee_rate, there is no need to disable again.
        let effective_base_rate = pool_state.get_effective_trade_fee_rate(&ctx.amm_config);
        pool_state.disable_decay_fee_if_needed(zero_for_one, decay_trade_fee_rate, effective_base_rate)?;
        trade_fee_rate
    };
    exact_internal_v2_with_fee_rate(
        ctx,
        remaining_accounts,
        amount_specified,
        sqrt_price_limit_x64,
        is_base_input,
        trade_fee_rate,
    )
}

pub fn exact_internal_v2_with_fee_rate<'c: 'info, 'info>(
    ctx: &mut SwapSingleV2<'info>,
    remaining_accounts: &'c [AccountInfo<'info>],
    amount_specified: u64,
    sqrt_price_limit_x64: u128,
    is_base_input: bool,
    trade_fee_rate: u32,
) -> Result<u64> {
    // invoke_memo_instruction(SWAP_MEMO_MSG, ctx.memo_program.to_account_info())?;

    let block_timestamp = solana_program::clock::Clock::get()?.unix_timestamp as u64;

    let amount_0;
    let amount_1;
    let zero_for_one;
    let swap_price_before;

    let input_balance_before = ctx.input_token_account.amount;
    let output_balance_before = ctx.output_token_account.amount;

    // calculate specified amount because the amount includes transfer_fee as input and without transfer_fee as output
    let (amount_calculate_specified, transfer_fee) = if is_base_input {
        let transfer_fee = util::get_transfer_fee(ctx.input_vault_mint.clone(), amount_specified).unwrap();
        (amount_specified - transfer_fee, transfer_fee)
    } else {
        let transfer_fee = util::get_transfer_inverse_fee(ctx.output_vault_mint.clone(), amount_specified).unwrap();
        (amount_specified + transfer_fee, transfer_fee)
    };

    {
        swap_price_before = ctx.pool_state.load()?.sqrt_price_x64;
        let pool_state = &mut ctx.pool_state.load_mut()?;
        zero_for_one = ctx.input_vault.mint == pool_state.token_mint_0;

        require_gt!(block_timestamp, pool_state.open_time);

        require!(
            if zero_for_one {
                ctx.input_vault.key() == pool_state.token_vault_0 && ctx.output_vault.key() == pool_state.token_vault_1
            } else {
                ctx.input_vault.key() == pool_state.token_vault_1 && ctx.output_vault.key() == pool_state.token_vault_0
            },
            ErrorCode::InvalidInputPoolVault
        );

        let mut tickarray_bitmap_extension = None;
        let tick_array_states = &mut VecDeque::new();

        let tick_array_bitmap_extension_key = TickArrayBitmapExtension::key(pool_state.key());
        for account_info in remaining_accounts.into_iter() {
            if account_info.key().eq(&tick_array_bitmap_extension_key) {
                tickarray_bitmap_extension = Some(
                    *(AccountLoader::<TickArrayBitmapExtension>::try_from(account_info)?
                        .load()?
                        .deref()),
                );
                continue;
            }
            // Tolerate unrelated accounts in remaining_accounts, as long as the tick_array entries are present.
            if let Ok(tick_array_container) = TickArrayContainer::load_data_mut(account_info) {
                tick_array_states.push_back(tick_array_container);
            }
        }

        let sqrt_price_limit = if sqrt_price_limit_x64 == 0 {
            if zero_for_one {
                tick_math::MIN_SQRT_PRICE_X64 + 1
            } else {
                tick_math::MAX_SQRT_PRICE_X64 - 1
            }
        } else {
            sqrt_price_limit_x64
        };

        (amount_0, amount_1) = swap_internal_with_fee_rate(
            &ctx.amm_config,
            pool_state,
            tick_array_states,
            &mut ctx.observation_state.load_mut()?,
            &tickarray_bitmap_extension,
            amount_calculate_specified,
            sqrt_price_limit,
            zero_for_one,
            is_base_input,
            oracle::block_timestamp(),
            trade_fee_rate,
        )?;

        #[cfg(feature = "enable-log")]
        msg!(
            "exact_swap_internal, is_base_input:{}, amount_0: {}, amount_1: {}",
            is_base_input,
            amount_0,
            amount_1
        );
        require!(amount_0 != 0 && amount_1 != 0, ErrorCode::TooSmallInputOrOutputAmount);
    }
    let (token_account_0, token_account_1, vault_0, vault_1, vault_0_mint, vault_1_mint) = if zero_for_one {
        (
            ctx.input_token_account.clone(),
            ctx.output_token_account.clone(),
            ctx.input_vault.clone(),
            ctx.output_vault.clone(),
            ctx.input_vault_mint.clone(),
            ctx.output_vault_mint.clone(),
        )
    } else {
        (
            ctx.output_token_account.clone(),
            ctx.input_token_account.clone(),
            ctx.output_vault.clone(),
            ctx.input_vault.clone(),
            ctx.output_vault_mint.clone(),
            ctx.input_vault_mint.clone(),
        )
    };

    // user or pool real amount delta without tranfer fee
    let amount_0_without_fee;
    let amount_1_without_fee;
    // the transfer fee amount charged by withheld_amount
    let transfer_fee_0;
    let transfer_fee_1;
    // transfer amount
    let transfer_amount_0;
    let transfer_amount_1;
    if zero_for_one {
        transfer_fee_0 = if is_base_input && amount_0 == amount_calculate_specified {
            transfer_fee
        } else {
            util::get_transfer_inverse_fee(vault_0_mint.clone(), amount_0).unwrap()
        };
        transfer_fee_1 = util::get_transfer_fee(vault_1_mint.clone(), amount_1).unwrap();

        amount_0_without_fee = amount_0;
        amount_1_without_fee = amount_1.checked_sub(transfer_fee_1).unwrap();
        (transfer_amount_0, transfer_amount_1) = (amount_0 + transfer_fee_0, amount_1);
        #[cfg(feature = "enable-log")]
        msg!(
            "amount_0:{}, transfer_fee_0:{}, amount_1:{}, transfer_fee_1:{}",
            amount_0,
            transfer_fee_0,
            amount_1,
            transfer_fee_1
        );
        //  x -> y, deposit x token from user to pool vault.
        transfer_from_user_to_pool_vault(
            &ctx.payer,
            &token_account_0.to_account_info(),
            &vault_0.to_account_info(),
            Some(vault_0_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_0,
        )?;
        if vault_1.amount <= transfer_amount_1 {
            // freeze pool, disable all instructions
            ctx.pool_state.load_mut()?.set_status(255);
        }
        // x -> y，transfer y token from pool vault to user.
        transfer_from_pool_vault_to_user(
            &ctx.pool_state,
            &vault_1.to_account_info(),
            &token_account_1.to_account_info(),
            Some(vault_1_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_1,
        )?;
    } else {
        transfer_fee_0 = util::get_transfer_fee(vault_0_mint.clone(), amount_0).unwrap();
        transfer_fee_1 = if is_base_input && amount_1 == amount_calculate_specified {
            transfer_fee
        } else {
            util::get_transfer_inverse_fee(vault_1_mint.clone(), amount_1).unwrap()
        };

        amount_0_without_fee = amount_0.checked_sub(transfer_fee_0).unwrap();
        amount_1_without_fee = amount_1;
        (transfer_amount_0, transfer_amount_1) = (amount_0, amount_1 + transfer_fee_1);
        #[cfg(feature = "enable-log")]
        msg!(
            "amount_0:{}, transfer_fee_0:{}, amount_1:{}, transfer_fee_1:{}",
            amount_0,
            transfer_fee_0,
            amount_1,
            transfer_fee_1
        );
        transfer_from_user_to_pool_vault(
            &ctx.payer,
            &token_account_1.to_account_info(),
            &vault_1.to_account_info(),
            Some(vault_1_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_1,
        )?;
        if vault_0.amount <= transfer_amount_0 {
            // freeze pool, disable all instructions
            ctx.pool_state.load_mut()?.set_status(255);
        }
        transfer_from_pool_vault_to_user(
            &ctx.pool_state,
            &vault_0.to_account_info(),
            &token_account_0.to_account_info(),
            Some(vault_0_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_0,
        )?;
    }
    ctx.output_token_account.reload()?;
    ctx.input_token_account.reload()?;

    let pool_state = ctx.pool_state.load()?;
    emit!(SwapEvent {
        pool_state: pool_state.key(),
        sender: ctx.payer.key(),
        token_account_0: token_account_0.key(),
        token_account_1: token_account_1.key(),
        amount_0: amount_0_without_fee,
        transfer_fee_0,
        amount_1: amount_1_without_fee,
        transfer_fee_1,
        zero_for_one,
        sqrt_price_x64: pool_state.sqrt_price_x64,
        liquidity: pool_state.liquidity,
        tick: pool_state.tick_current
    });
    if zero_for_one {
        require_gt!(swap_price_before, pool_state.sqrt_price_x64);
    } else {
        require_gt!(pool_state.sqrt_price_x64, swap_price_before);
    }
    if sqrt_price_limit_x64 == 0 {
        // Does't allow partial filled without specified limit_price.
        if is_base_input {
            if zero_for_one {
                require_eq!(amount_specified, transfer_amount_0);
            } else {
                require_eq!(amount_specified, transfer_amount_1);
            }
        } else {
            if zero_for_one {
                require_eq!(amount_specified, transfer_amount_1);
            } else {
                require_eq!(amount_specified, transfer_amount_0);
            }
        }
    }

    if is_base_input {
        Ok(ctx
            .output_token_account
            .amount
            .checked_sub(output_balance_before)
            .unwrap())
    } else {
        Ok(input_balance_before
            .checked_sub(ctx.input_token_account.amount)
            .unwrap())
    }
}

pub fn swap_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, SwapSingleV2<'info>>,
    amount: u64,
    other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
    is_base_input: bool,
) -> Result<()> {
    if ctx.accounts.pool_state.load()?.is_swap_dynamic_fee_enabled() {
        return err!(ErrorCode::SwapDynamicFeeEnabled);
    }

    let amount_result = exact_internal_v2(
        ctx.accounts,
        ctx.remaining_accounts,
        amount,
        sqrt_price_limit_x64,
        is_base_input,
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
