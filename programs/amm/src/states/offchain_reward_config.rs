use crate::error::ErrorCode;
use anchor_lang::{prelude::*, system_program};

pub const OFFCHAIN_REWARD_SEED: &str = "offchain_reward";

/// Holds the current owner of the factory
#[account]
#[derive(Default, Debug)]
pub struct OffchainRewardConfig {
    /// the pool state address
    pub pool_id: Pubkey,

    /// the vault to hold the reward
    /// vault address is this config address
    pub reward_vault: Pubkey,

    /// Bump to identify vault PDA
    pub vault_bump: [u8; 1],

    /// reward token mint address list
    /// reward token account is ATA(reward_vault, reward_mint)
    pub reward_mint_vec: Vec<Pubkey>,
}

impl OffchainRewardConfig {
    const BASE_LENGTH: usize = 8 + 32 + 32 + 1 + 4;

    /// if store count of reward_mint_vec, the length of this account is BASE_LENGTH + count * 32
    pub fn need_len(count: usize) -> usize {
        Self::BASE_LENGTH + count * 32 // 32 for each Pubkey in reward_mint_vec
    }

    /// real length of this account
    pub fn real_len(&self) -> usize {
        Self::BASE_LENGTH + self.reward_mint_vec.len() * 32 // 32 for each Pubkey in reward_mint_vec
    }

    pub fn initialize(&mut self, pool_id: Pubkey, reward_vault: Pubkey, vault_bump: u8) -> Result<()> {
        self.pool_id = pool_id;
        self.reward_vault = reward_vault;
        self.vault_bump = [vault_bump];

        Ok(())
    }

    /// Get the seeds used to derive the PDA for this offchain reward config.
    pub fn seeds(&self) -> [&[u8]; 3] {
        [
            OFFCHAIN_REWARD_SEED.as_bytes(),
            self.pool_id.as_ref(),
            self.vault_bump.as_ref(),
        ]
    }

    /// Check if the reward-config account space needs to be reallocated to add a mint account.
    /// Returns `true` if the account was reallocated.
    pub fn realloc_if_needed<'a>(
        reward_config: AccountInfo<'a>,
        reward_mint_count: usize,
        rent_payer: AccountInfo<'a>,
        system_program: AccountInfo<'a>,
    ) -> Result<bool> {
        // Sanity checks
        require_keys_eq!(*reward_config.owner, crate::id(), ErrorCode::IllegalAccountOwner);

        let current_account_size = reward_config.data.borrow().len();
        let account_size_to_fit_members = Self::need_len(reward_mint_count);

        // Check if we need to reallocate space.
        if current_account_size >= account_size_to_fit_members {
            return Ok(false);
        }

        let new_size = std::cmp::max(
            current_account_size + (10 * 32), // We need to allocate more space. To avoid doing this operation too often, we increment it by 10 pubkey.
            account_size_to_fit_members,
        );
        // Reallocate more space.
        AccountInfo::resize(&reward_config, new_size)?;

        // If more lamports are needed, transfer them to the account.
        let rent_exempt_lamports = Rent::get().unwrap().minimum_balance(new_size).max(1);
        let top_up_lamports = rent_exempt_lamports.saturating_sub(reward_config.to_account_info().lamports());

        if top_up_lamports > 0 {
            require_keys_eq!(*system_program.key, system_program::ID, ErrorCode::InvalidAccount);

            system_program::transfer(
                CpiContext::new(
                    system_program,
                    system_program::Transfer {
                        from: rent_payer,
                        to: reward_config,
                    },
                ),
                top_up_lamports,
            )?;
        }

        Ok(true)
    }

    /// Add a new reward mint to the configuration.
    pub fn add_reward_mint(&mut self, reward_mint: Pubkey) -> Result<bool> {
        if self.reward_mint_vec.contains(&reward_mint) {
            return Ok(false); // No need to add if it already exists
        }
        self.reward_mint_vec.push(reward_mint);
        Ok(true)
    }

    /// Remove a reward mint from the configuration.
    pub fn remove_reward_mint(&mut self, reward_mint: Pubkey) -> Result<bool> {
        if let Some(pos) = self.reward_mint_vec.iter().position(|x| *x == reward_mint) {
            self.reward_mint_vec.remove(pos);
            Ok(true)
        } else {
            Ok(false) // No need to remove if it doesn't exist
        }
    }
}
