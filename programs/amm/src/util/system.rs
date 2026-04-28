use crate::error::ErrorCode as ClmmErrorCode;
use anchor_lang::{prelude::*, system_program};

pub fn create_or_allocate_account<'a>(
    program_id: &Pubkey,
    payer: AccountInfo<'a>,
    system_program: AccountInfo<'a>,
    target_account: AccountInfo<'a>,
    siger_seed: &[&[u8]],
    space: usize,
) -> Result<()> {
    let rent = Rent::get()?;
    let current_lamports = target_account.lamports();

    #[cfg(all(feature = "localnet", feature = "enable-log"))]
    msg!(
        "create_or_allocate_account, target_account: {}, current_lamports: {}, cur_space:{}, target_space: {}",
        target_account.key.to_string(),
        current_lamports,
        target_account.data_len(),
        space
    );

    if current_lamports == 0 {
        let lamports = rent.minimum_balance(space);
        let cpi_accounts = system_program::CreateAccount {
            from: payer,
            to: target_account.clone(),
        };
        let cpi_context = CpiContext::new(system_program.clone(), cpi_accounts);
        system_program::create_account(
            cpi_context.with_signer(&[siger_seed]),
            lamports,
            u64::try_from(space).unwrap(),
            program_id,
        )?;
    } else {
        let required_lamports = rent.minimum_balance(space).max(1).saturating_sub(current_lamports);
        if required_lamports > 0 {
            let cpi_accounts = system_program::Transfer {
                from: payer.to_account_info(),
                to: target_account.clone(),
            };
            let cpi_context = CpiContext::new(system_program.clone(), cpi_accounts);
            system_program::transfer(cpi_context, required_lamports)?;
        }
        let cpi_accounts = system_program::Allocate {
            account_to_allocate: target_account.clone(),
        };
        let cpi_context = CpiContext::new(system_program.clone(), cpi_accounts);
        system_program::allocate(cpi_context.with_signer(&[siger_seed]), u64::try_from(space).unwrap())?;

        let cpi_accounts = system_program::Assign {
            account_to_assign: target_account.clone(),
        };
        let cpi_context = CpiContext::new(system_program.clone(), cpi_accounts);
        system_program::assign(cpi_context.with_signer(&[siger_seed]), program_id)?;
    }
    Ok(())
}

/// Check if the target account space needs to be reallocated to fit the new_account_space.
/// Returns `true` if the account was reallocated.
pub fn realloc_account_if_needed<'a>(
    target_account: &AccountInfo<'a>,
    new_account_space: usize,
    rent_payer: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
) -> Result<bool> {
    // Sanity checks
    require_keys_eq!(*target_account.owner, crate::id(), ClmmErrorCode::IllegalAccountOwner);

    let current_account_size = target_account.data.borrow().len();

    // Check if we need to reallocate space.
    if current_account_size >= new_account_space {
        return Ok(false);
    }

    // Reallocate more space.
    AccountInfo::resize(target_account, new_account_space)?;

    // If more lamports are needed, transfer them to the account.
    let rent_exempt_lamports = Rent::get().unwrap().minimum_balance(new_account_space).max(1);
    let top_up_lamports = rent_exempt_lamports.saturating_sub(target_account.to_account_info().lamports());

    if top_up_lamports > 0 {
        require_keys_eq!(*system_program.key, system_program::ID, ClmmErrorCode::InvalidAccount);

        system_program::transfer(
            CpiContext::new(
                system_program.clone(),
                system_program::Transfer {
                    from: rent_payer.clone(),
                    to: target_account.clone(),
                },
            ),
            top_up_lamports,
        )?;
    }

    Ok(true)
}

#[cfg(not(any(test, feature = "client")))]
pub fn get_recent_epoch() -> Result<u64> {
    Ok(Clock::get()?.epoch)
}

#[cfg(any(test, feature = "client"))]
pub fn get_recent_epoch() -> Result<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() / (2 * 24 * 3600))
}
