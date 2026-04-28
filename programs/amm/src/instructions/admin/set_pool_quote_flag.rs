use crate::error::ErrorCode;
use crate::states::*;
use anchor_lang::prelude::*;

#[derive(Accounts)]
pub struct SetPoolQuoteFlag<'info> {
    /// Must be the pool_manager authority
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

/// Set the quote-token flag for the pool.
/// `token1_as_quote`: true if token1 is the quote token, false if token0 is the quote token.
pub fn set_pool_quote_flag(ctx: Context<SetPoolQuoteFlag>, token1_as_quote: bool) -> Result<()> {
    let mut pool_state = ctx.accounts.pool_state.load_mut()?;
    pool_state.set_quote_token_flag(token1_as_quote);
    Ok(())
}
