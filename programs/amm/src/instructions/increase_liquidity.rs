use super::add_liquidity;
use crate::error::ErrorCode;
use crate::instructions::LiquidityChangeResult;
use crate::libraries::{big_num::U128, fixed_point_64, full_math::MulDiv};
use crate::states::*;
use crate::util::*;
use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use anchor_spl::token_interface::{Mint, Token2022};

#[derive(Accounts)]
pub struct IncreaseLiquidity<'info> {
    /// Pays to mint the position
    pub nft_owner: Signer<'info>,

    /// The token account for nft
    #[account(
        constraint = nft_account.mint == personal_position.nft_mint,
        constraint = nft_account.amount == 1,
        token::authority = nft_owner
    )]
    pub nft_account: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// CHECK: Deprecated: protocol_position is deprecated and kept for compatibility.
    pub protocol_position: UncheckedAccount<'info>,

    /// Increase liquidity for this position
    #[account(mut, constraint = personal_position.pool_id == pool_state.key())]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    /// CHECK: both support fix-tick-array and dynamic-tick-array
    /// Stores init state for the lower tick
    /// constraint = tick_array_lower.load()?.pool_id == pool_state.key()
    #[account(mut)]
    pub tick_array_lower: UncheckedAccount<'info>,

    /// CHECK: both support fix-tick-array and dynamic-tick-array
    /// Stores init state for the upper tick
    /// constraint = tick_array_upper.load()?.pool_id == pool_state.key()
    #[account(mut)]
    pub tick_array_upper: UncheckedAccount<'info>,

    /// The payer's token account for token_0
    #[account(
        mut,
        token::mint = token_vault_0.mint
    )]
    pub token_account_0: Box<Account<'info, TokenAccount>>,

    /// The token account spending token_1 to mint the position
    #[account(
        mut,
        token::mint = token_vault_1.mint
    )]
    pub token_account_1: Box<Account<'info, TokenAccount>>,

    /// The address that holds pool tokens for token_0
    #[account(
        mut,
        constraint = token_vault_0.key() == pool_state.load()?.token_vault_0
    )]
    pub token_vault_0: Box<Account<'info, TokenAccount>>,

    /// The address that holds pool tokens for token_1
    #[account(
        mut,
        constraint = token_vault_1.key() == pool_state.load()?.token_vault_1
    )]
    pub token_vault_1: Box<Account<'info, TokenAccount>>,

    /// Program to create mint account and mint tokens
    pub token_program: Program<'info, Token>,
    // remaining account
    // #[account(
    //     seeds = [
    //         POOL_TICK_ARRAY_BITMAP_SEED.as_bytes(),
    //         pool_state.key().as_ref(),
    //     ],
    //     bump
    // )]
    // pub tick_array_bitmap: AccountLoader<'info, TickArrayBitmapExtension>,
}

pub fn increase_liquidity_v1<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, IncreaseLiquidity<'info>>,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    base_flag: Option<bool>,
) -> Result<()> {
    increase_liquidity(
        &ctx.accounts.nft_owner,
        &ctx.accounts.pool_state,
        &mut ctx.accounts.personal_position,
        &ctx.accounts.tick_array_lower.to_account_info(),
        &ctx.accounts.tick_array_upper.to_account_info(),
        &ctx.accounts.token_account_0.to_account_info(),
        &ctx.accounts.token_account_1.to_account_info(),
        &ctx.accounts.token_vault_0.to_account_info(),
        &ctx.accounts.token_vault_1.to_account_info(),
        &ctx.accounts.token_program,
        None,
        None,
        None,
        &ctx.remaining_accounts,
        liquidity,
        amount_0_max,
        amount_1_max,
        base_flag,
    )
}

pub fn increase_liquidity<'a, 'b, 'c: 'info, 'info>(
    nft_owner: &'b Signer<'info>,
    pool_state_loader: &'b AccountLoader<'info, PoolState>,
    personal_position: &'b mut Box<Account<'info, PersonalPositionState>>,
    tick_array_lower_account: &'b AccountInfo<'info>,
    tick_array_upper_account: &'b AccountInfo<'info>,
    token_account_0: &'b AccountInfo<'info>,
    token_account_1: &'b AccountInfo<'info>,
    token_vault_0: &'b AccountInfo<'info>,
    token_vault_1: &'b AccountInfo<'info>,
    token_program: &'b Program<'info, Token>,
    token_program_2022: Option<&Program<'info, Token2022>>,
    vault_0_mint: Option<Box<InterfaceAccount<'info, Mint>>>,
    vault_1_mint: Option<Box<InterfaceAccount<'info, Mint>>>,

    remaining_accounts: &'c [AccountInfo<'info>],
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    base_flag: Option<bool>,
) -> Result<()> {
    let mut liquidity = liquidity;
    let pool_state = &mut pool_state_loader.load_mut()?;
    if !pool_state.get_status_by_bit(PoolStatusBitIndex::OpenPositionOrIncreaseLiquidity) {
        return err!(ErrorCode::NotApproved);
    }

    let tick_spacing = pool_state.tick_spacing;
    let tick_lower = personal_position.tick_lower_index;
    let tick_upper = personal_position.tick_upper_index;

    let tick_array_lower_loader = TickArrayContainer::try_from(&tick_array_lower_account, tick_lower, tick_spacing)?;
    let tick_array_upper_loader = TickArrayContainer::try_from(&tick_array_upper_account, tick_upper, tick_spacing)?;

    // check tick array pool id
    require_keys_eq!(tick_array_lower_loader.get_pool_id()?, pool_state.key());
    require_keys_eq!(tick_array_upper_loader.get_pool_id()?, pool_state.key());

    let use_tickarray_bitmap_extension = pool_state.is_overflow_default_tickarray_bitmap(vec![tick_lower, tick_upper]);

    let LiquidityChangeResult {
        amount_0,
        amount_1,
        amount_0_transfer_fee,
        amount_1_transfer_fee,
        fee_growth_inside_0_x64: fee_growth_inside_0_x64_latest,
        fee_growth_inside_1_x64: fee_growth_inside_1_x64_latest,
        reward_growths_inside: reward_growths_inside_latest,
        ..
    } = add_liquidity(
        &nft_owner,
        token_account_0,
        token_account_1,
        token_vault_0,
        token_vault_1,
        &tick_array_lower_loader,
        &tick_array_upper_loader,
        token_program_2022,
        token_program,
        vault_0_mint,
        vault_1_mint,
        if use_tickarray_bitmap_extension {
            require_keys_eq!(
                remaining_accounts[0].key(),
                TickArrayBitmapExtension::key(pool_state_loader.key())
            );
            Some(&remaining_accounts[0])
        } else {
            None
        },
        pool_state,
        &mut liquidity,
        amount_0_max,
        amount_1_max,
        tick_lower,
        tick_upper,
        base_flag,
    )?;

    personal_position.increase_liquidity(
        liquidity,
        fee_growth_inside_0_x64_latest,
        fee_growth_inside_1_x64_latest,
        reward_growths_inside_latest,
        get_recent_epoch()?,
    )?;
    emit!(IncreaseLiquidityEvent {
        position_nft_mint: personal_position.nft_mint,
        liquidity,
        amount_0,
        amount_1,
        amount_0_transfer_fee,
        amount_1_transfer_fee
    });

    Ok(())
}

pub fn calculate_latest_token_fees(
    last_total_fees: u64,
    fee_growth_inside_last_x64: u128,
    fee_growth_inside_latest_x64: u128,
    liquidity: u128,
) -> u64 {
    let fee_growth_delta = U128::from(fee_growth_inside_latest_x64.wrapping_sub(fee_growth_inside_last_x64))
        .mul_div_floor(U128::from(liquidity), U128::from(fixed_point_64::Q64))
        .unwrap()
        .to_underflow_u64();
    #[cfg(feature = "enable-log")]
    msg!("calculate_latest_token_fees fee_growth_delta:{}, fee_growth_inside_latest_x64:{}, fee_growth_inside_last_x64:{}, liquidity:{}", fee_growth_delta, fee_growth_inside_latest_x64, fee_growth_inside_last_x64, liquidity);
    last_total_fees.checked_add(fee_growth_delta).unwrap()
}
