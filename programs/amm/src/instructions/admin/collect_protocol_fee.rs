use crate::decrease_liquidity::check_unclaimed_fees_and_vault;
use crate::error::ErrorCode as ClmmErrorCode;
use crate::states::*;
use crate::util::*;
use anchor_lang::prelude::*;
use anchor_spl::associated_token::get_associated_token_address_with_program_id;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};

#[derive(Accounts)]
pub struct CollectProtocolFee<'info> {
    /// amm admin group account to store admin permissions.
    /// anyone can collect fee, but only fee-manager in admin group can receive fee
    #[account(
        seeds = [
            ADMIN_GROUP_SEED.as_bytes()
        ],
        bump,
    )]
    pub admin_group: Box<Account<'info, AmmAdminGroup>>,

    /// Pool state stores accumulated protocol fee amount
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// The address that holds pool tokens for token_0
    #[account(
        mut,
        constraint = token_vault_0.key() == pool_state.load()?.token_vault_0
    )]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The address that holds pool tokens for token_1
    #[account(
        mut,
        constraint = token_vault_1.key() == pool_state.load()?.token_vault_1
    )]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The mint of token vault 0
    #[account(
        address = token_vault_0.mint
    )]
    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint of token vault 1
    #[account(
        address = token_vault_1.mint
    )]
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The address that receives the collected token_0 protocol fees
    #[account(mut)]
    pub recipient_token_account_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The address that receives the collected token_1 protocol fees
    #[account(mut)]
    pub recipient_token_account_1: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The SPL program to perform token transfers
    pub token_program: Program<'info, Token>,

    /// The SPL program 2022 to perform token transfers
    pub token_program_2022: Program<'info, Token2022>,

    pub associated_token_program: Program<'info, anchor_spl::associated_token::AssociatedToken>,
}

pub fn collect_protocol_fee(
    ctx: Context<CollectProtocolFee>,
    amount_0_requested: u64,
    amount_1_requested: u64,
) -> Result<()> {
    // check if the admin group is valid
    ctx.accounts.admin_group.validate()?;

    {
        let expected_recipient_token_account_0 = get_associated_token_address_with_program_id(
            &ctx.accounts.admin_group.fee_keeper,
            &ctx.accounts.vault_0_mint.key(),
            ctx.accounts.vault_0_mint.to_account_info().owner,
        );
        require_keys_eq!(
            ctx.accounts.recipient_token_account_0.key(),
            expected_recipient_token_account_0,
            ClmmErrorCode::InvalidAccount
        );

        let expected_recipient_token_account_1 = get_associated_token_address_with_program_id(
            &ctx.accounts.admin_group.fee_keeper,
            &ctx.accounts.vault_1_mint.key(),
            ctx.accounts.vault_1_mint.to_account_info().owner,
        );
        require_keys_eq!(
            ctx.accounts.recipient_token_account_1.key(),
            expected_recipient_token_account_1,
            ClmmErrorCode::InvalidAccount
        );
    }

    let amount_0: u64;
    let amount_1: u64;
    {
        let mut pool_state = ctx.accounts.pool_state.load_mut()?;

        amount_0 = amount_0_requested.min(pool_state.protocol_fees_token_0);
        amount_1 = amount_1_requested.min(pool_state.protocol_fees_token_1);

        pool_state.protocol_fees_token_0 = pool_state.protocol_fees_token_0.checked_sub(amount_0).unwrap();
        pool_state.protocol_fees_token_1 = pool_state.protocol_fees_token_1.checked_sub(amount_1).unwrap();
    }
    transfer_from_pool_vault_to_user(
        &ctx.accounts.pool_state,
        &ctx.accounts.token_vault_0.to_account_info(),
        &ctx.accounts.recipient_token_account_0.to_account_info(),
        Some(ctx.accounts.vault_0_mint.clone()),
        &ctx.accounts.token_program,
        Some(ctx.accounts.token_program_2022.to_account_info()),
        amount_0,
    )?;

    transfer_from_pool_vault_to_user(
        &ctx.accounts.pool_state,
        &ctx.accounts.token_vault_1.to_account_info(),
        &ctx.accounts.recipient_token_account_1.to_account_info(),
        Some(ctx.accounts.vault_1_mint.clone()),
        &ctx.accounts.token_program,
        Some(ctx.accounts.token_program_2022.to_account_info()),
        amount_1,
    )?;

    check_unclaimed_fees_and_vault(
        &ctx.accounts.pool_state,
        &ctx.accounts.token_vault_0.to_account_info(),
        &ctx.accounts.token_vault_1.to_account_info(),
    )?;

    emit!(CollectProtocolFeeEvent {
        pool_state: ctx.accounts.pool_state.key(),
        recipient_token_account_0: ctx.accounts.recipient_token_account_0.key(),
        recipient_token_account_1: ctx.accounts.recipient_token_account_1.key(),
        amount_0,
        amount_1,
    });

    Ok(())
}
