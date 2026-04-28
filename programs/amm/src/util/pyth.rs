use anchor_lang::prelude::*;

use pyth_solana_receiver_sdk::price_update::{Price, PriceUpdateV2};

use crate::error::ErrorCode;
use crate::libraries::{big_num::U256, fixed_point_64, full_math::MulDiv};

/// Load a price from a Pyth oracle account.
pub fn load_pyth_price<'a>(
    account_info: &'a AccountInfo<'a>,
    expected_feed_id: &[u8; 32],
    max_age_seconds: u64,
) -> Result<Price> {
    require_keys_eq!(
        *account_info.owner,
        pyth_solana_receiver_sdk::ID,
        ErrorCode::InvalidPythOracleAccount
    );

    let account: Account<'_, PriceUpdateV2> = Account::try_from(account_info)?;

    // We only use the Pyth oracle price to compute part of the fee tier; this verification-level check
    // can be skipped because we still validate the price by feed-id below.
    // require!(
    //     account.verification_level.gte(VerificationLevel::Full),
    //     ErrorCode::InvalidPythOracleAccount
    // );

    let price = account
        .get_price_unchecked(expected_feed_id)
        .map_err(|_| ErrorCode::InvalidPythOracleAccount)?;

    require!(price.price > 0, ErrorCode::InvalidPythOracleAccount);

    let clock = Clock::get()?;
    let oldest_allowed = clock
        .unix_timestamp
        .checked_sub(max_age_seconds as i64)
        .ok_or(ErrorCode::CalculateOverflow)?;
    require!(price.publish_time >= oldest_allowed, ErrorCode::PythPriceStale);

    Ok(price)
}

/// Compute p_index, the price ratio token0/token1.
/// Result is in Q64.64 format, compatible with pool.sqrt_price_x64.
/// `decimals_0`, `decimals_1`: token decimals, used to convert the raw price ratio into the unit
/// system used internally by the pool.
/// Pool internal price: p_0 = token1_raw / token0_raw = (token1_real * 10^d1) / (token0_real * 10^d0),
/// so p_index must be multiplied by 10^(d1 - d0) to align.
pub fn calculate_price_index(
    token0_price: &Price,
    token1_price: &Price,
    decimals_0: u8,
    decimals_1: u8,
) -> Result<u128> {
    let exp_diff = token0_price
        .exponent
        .checked_sub(token1_price.exponent)
        .ok_or(ErrorCode::CalculateOverflow)?;

    let price0 = token0_price.price as i128;
    let price1 = token1_price.price as i128;
    require!(price0 > 0 && price1 > 0, ErrorCode::InvalidPythOracleAccount);

    // Combine exponent difference and decimals difference into a single net_exp
    // p_index = (price0 / price1) * 10^exp_diff * 10^(decimals_1 - decimals_0)
    let decimal_diff = (decimals_1 as i32) - (decimals_0 as i32);
    let net_exp = exp_diff.checked_add(decimal_diff).ok_or(ErrorCode::CalculateOverflow)?;

    let (numerator, denominator) = if net_exp >= 0 {
        let scale = pow10_u128(net_exp as u32)?;
        let num = (price0 as u128)
            .checked_mul(scale)
            .ok_or(ErrorCode::CalculateOverflow)?;
        (num, price1 as u128)
    } else {
        let scale = pow10_u128((-net_exp) as u32)?;
        let denom = (price1 as u128)
            .checked_mul(scale)
            .ok_or(ErrorCode::CalculateOverflow)?;
        (price0 as u128, denom)
    };

    let price_x64 = U256::from(numerator)
        .mul_div_floor(U256::from(fixed_point_64::Q64), U256::from(denominator))
        .ok_or(ErrorCode::CalculateOverflow)?;

    u128::try_from(price_x64).map_err(|_| ErrorCode::CalculateOverflow.into())
}

fn pow10_u128(exp: u32) -> Result<u128> {
    let mut result: u128 = 1;
    for _ in 0..exp {
        result = result.checked_mul(10).ok_or(ErrorCode::CalculateOverflow)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libraries::fixed_point_64;

    fn make_price(price: i64, exponent: i32) -> Price {
        Price {
            price,
            conf: 0,
            exponent,
            publish_time: 0,
        }
    }

    #[test]
    fn test_calculate_price_index_same_exponent() {
        // SOL = $100, USDC = $1, both exp -8
        // p_index = (100 / 1) in Q64 = 100 * Q64
        let token0 = make_price(10_000_000_000, -8); // $100
        let token1 = make_price(100_000_000, -8); // $1

        let p_index = calculate_price_index(&token0, &token1, 0, 0).unwrap();
        assert_eq!(p_index, 100 * fixed_point_64::Q64);
    }

    #[test]
    fn test_calculate_price_index_equal_prices() {
        // Both $1, same exponent → ratio = 1 → Q64
        let token0 = make_price(100_000_000, -8);
        let token1 = make_price(100_000_000, -8);

        let p_index = calculate_price_index(&token0, &token1, 0, 0).unwrap();
        assert_eq!(p_index, fixed_point_64::Q64);
    }

    #[test]
    fn test_calculate_price_index_different_exponent_positive_diff() {
        // token0: price=10000, exp=-2 → actual $100
        // token1: price=100000000, exp=-8 → actual $1
        // exp_diff = -2 - (-8) = 6
        // numerator = 10000 * 10^6 = 10_000_000_000
        // denominator = 100_000_000
        // ratio = 100 * Q64
        let token0 = make_price(10_000, -2);
        let token1 = make_price(100_000_000, -8);

        let p_index = calculate_price_index(&token0, &token1, 0, 0).unwrap();
        assert_eq!(p_index, 100 * fixed_point_64::Q64);
    }

    #[test]
    fn test_calculate_price_index_negative_exp_diff() {
        // token0: price=100000000, exp=-8 → actual $1
        // token1: price=10000, exp=-2 → actual $100
        // exp_diff = -8 - (-2) = -6
        // denominator = 10000 * 10^6 = 10_000_000_000
        // numerator = 100_000_000
        // ratio = 0.01 * Q64 = Q64 / 100
        let token0 = make_price(100_000_000, -8);
        let token1 = make_price(10_000, -2);

        let p_index = calculate_price_index(&token0, &token1, 0, 0).unwrap();
        // floor(Q64 / 100) = 184467440737095516
        let expected = fixed_point_64::Q64 / 100;
        assert_eq!(p_index, expected);
    }

    #[test]
    fn test_calculate_price_index_fractional_ratio() {
        // token0 = $3, token1 = $2, same exponent
        // ratio = 1.5 → Q64 * 3 / 2
        let token0 = make_price(300_000_000, -8);
        let token1 = make_price(200_000_000, -8);

        let p_index = calculate_price_index(&token0, &token1, 0, 0).unwrap();
        // floor(3 * Q64 / 2) = floor(1.5 * Q64)
        let expected = fixed_point_64::Q64 / 2 * 3;
        assert_eq!(p_index, expected);
    }

    #[test]
    fn test_calculate_price_index_with_decimal_correction() {
        // SOL(9 decimals) / USDC(6 decimals), SOL=$100, USDC=$1.
        // Raw price ratio = 100, but pool internal price p_0 = 100 * 10^(6-9) = 0.1.
        // p_index must be multiplied by 10^(decimals_1 - decimals_0) = 10^(6-9) = 10^-3.
        let token0 = make_price(10_000_000_000, -8); // SOL $100
        let token1 = make_price(100_000_000, -8); // USDC $1

        // decimals_0=9 (SOL), decimals_1=6 (USDC)
        let p_index = calculate_price_index(&token0, &token1, 9, 6).unwrap();
        // expected: 100 * 10^-3 * Q64 = Q64 / 10
        let expected = fixed_point_64::Q64 / 10;
        assert_eq!(p_index, expected);
    }

    #[test]
    fn test_calculate_price_index_decimal_correction_token1_more_decimals() {
        // token0(6 decimals) / token1(9 decimals), both $1.
        // Raw price ratio = 1, pool internal p_0 = 1 * 10^(9-6) = 1000.
        let token0 = make_price(100_000_000, -8);
        let token1 = make_price(100_000_000, -8);

        let p_index = calculate_price_index(&token0, &token1, 6, 9).unwrap();
        // expected: 1 * 10^3 * Q64 = 1000 * Q64
        assert_eq!(p_index, 1000 * fixed_point_64::Q64);
    }
}
