use crate::error::ErrorCode;
use crate::libraries::big_num::{U128, U256};
use crate::libraries::fixed_point_64;
use crate::libraries::full_math::MulDiv;
use crate::states::config::FEE_RATE_DENOMINATOR_VALUE;
use anchor_lang::prelude::*;

/// Compute arbitrage_fee.
/// `p_0`: pre-trade price (Q64.64).
/// `p_index`: oracle price (Q64.64).
/// `buffer_ppm`: buffer value (10^-6); same unit as FEE_RATE_DENOMINATOR_VALUE.
/// `fee_base`: base fee rate, also in 10^-6 (FEE_RATE_DENOMINATOR_VALUE).
/// Returns: arbitrage_fee (a fee rate in the same unit as `fee_base`).
pub fn calculate_arbitrage_fee(p_0: u128, p_index: u128, buffer_ppm: u16, fee_base: u32) -> Result<u32> {
    require!(p_index > 0, ErrorCode::InvalidSwapDynamicFeeParams);

    let diff = if p_0 >= p_index { p_0 - p_index } else { p_index - p_0 };
    let diff_ppm = U128::from(diff)
        .mul_div_ceil(U128::from(FEE_RATE_DENOMINATOR_VALUE as u128), U128::from(p_index))
        .ok_or(ErrorCode::CalculateOverflow)?
        .as_u128();

    let buffer = buffer_ppm as u128;
    let fee_base_u128 = fee_base as u128;
    if diff_ppm <= buffer + fee_base_u128 {
        return Ok(0);
    }

    let fee = diff_ppm - buffer - fee_base_u128;
    require!(fee <= u32::MAX as u128, ErrorCode::CalculateOverflow);
    Ok(fee as u32)
}

/// Compute trade_slippage_fee.
/// `trade_size`: trade size (normalized quote amount).
/// `base`: base coefficient (in units of 1/1000); base=5 means 5/1000 = 0.005.
/// `threshold`: threshold (in units of 100 * 10^decimals quote amounts); threshold=1 means 100 tokens.
/// Returns: trade_slippage_fee (a fee rate).
pub fn calculate_trade_slippage_fee(trade_size: u64, base: u8, threshold: u8) -> Result<u32> {
    let threshold = (threshold as u64)
        .checked_mul(100)
        .ok_or(ErrorCode::CalculateOverflow)?;
    if trade_size <= threshold {
        return Ok(0);
    }

    // To reduce error, multiply delta by FEE_RATE_DENOMINATOR_VALUE^2 before taking the square root;
    // the resulting fee is then automatically expressed in units of FEE_RATE_DENOMINATOR_VALUE.
    let delta = ((trade_size - threshold) as u128)
        .checked_mul(FEE_RATE_DENOMINATOR_VALUE as u128)
        .and_then(|v| v.checked_mul(FEE_RATE_DENOMINATOR_VALUE as u128))
        .ok_or(ErrorCode::CalculateOverflow)?;

    let sqrt_delta = integer_sqrt_u128(delta);

    let fee = U128::from(base as u128)
        .mul_div_ceil(sqrt_delta.into(), U128::from(1_000u128))
        .ok_or(ErrorCode::CalculateOverflow)?
        .as_u128();

    require!(fee <= u32::MAX as u128, ErrorCode::CalculateOverflow);

    Ok(fee as u32)
}

/// Compute imbalance_fee.
/// `quote_value_of_base`: quote-denominated value of base tokens at the current price.
/// `quote_balance`: amount of quote tokens held by the pool.
/// `base`: base coefficient (in units of 1/10); base=5 means 5/10 = 0.5.
/// `x`: threshold (in units of 1/100); x=10 means 10/100 = 0.1.
/// `is_buying_base`: whether the swap removes base from the pool (i.e., the user is buying base).
/// Returns: imbalance_fee (a fee rate).
pub fn calculate_imbalance_fee(
    quote_value_of_base: u128,
    quote_balance: u128,
    base: u8,
    x: u8,
    is_buying_base: bool,
) -> Result<u32> {
    let total_value = quote_value_of_base
        .checked_add(quote_balance)
        .ok_or(ErrorCode::CalculateOverflow)?;

    if total_value == 0 {
        return Ok(0);
    }

    // Decide whether to penalize based on swap direction.
    if quote_value_of_base > quote_balance {
        // base side is over-weighted: selling base worsens the imbalance; buying base eases it.
        if is_buying_base {
            return Ok(0);
        }
    } else if quote_value_of_base < quote_balance {
        // base side is under-weighted: buying base worsens the imbalance; selling base eases it.
        if !is_buying_base {
            return Ok(0);
        }
    } else {
        return Ok(0);
    }

    let diff = if quote_value_of_base >= quote_balance {
        quote_value_of_base - quote_balance
    } else {
        quote_balance - quote_value_of_base
    };

    let imbalance_ppm = U128::from(diff)
        .mul_div_ceil(U128::from(FEE_RATE_DENOMINATOR_VALUE as u128), U128::from(total_value))
        .ok_or(ErrorCode::CalculateOverflow)?
        .as_u128();

    // x is expressed in units of 1/100; multiply by 10_000 to convert to ppm.
    let x_ppm = (x as u128).checked_mul(10_000).ok_or(ErrorCode::CalculateOverflow)?;
    if imbalance_ppm <= x_ppm {
        return Ok(0);
    }

    let over = imbalance_ppm - x_ppm;
    let fee = U128::from(over)
        .mul_div_ceil(U128::from(base as u128), U128::from(10u128))
        .ok_or(ErrorCode::CalculateOverflow)?
        .as_u128();

    require!(fee <= u32::MAX as u128, ErrorCode::CalculateOverflow);

    Ok(fee as u32)
}

/// Compute the total swap-dynamic-fee (excluding base-fee).
#[inline(always)]
pub fn calculate_total_dynamic_fee(arbitrage_fee: u32, trade_slippage_fee: u32, imbalance_fee: u32) -> Result<u32> {
    let total = (arbitrage_fee as u64)
        .checked_add(trade_slippage_fee as u64)
        .and_then(|v| v.checked_add(imbalance_fee as u64))
        .ok_or(ErrorCode::CalculateOverflow)?;
    require!(total <= u32::MAX as u64, ErrorCode::CalculateOverflow);
    Ok(total as u32)
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicFeeInputs {
    pub p_0: u128,
    pub p_index: u128,
    pub trade_size: u64,
    pub quote_value_of_base: u128,
    pub quote_balance: u128,
    pub is_buying_base: bool,
    pub fee_base: u32,
    pub arbitrage_fee_buffer_ppm: u16,
    pub trade_slippage_fee_base: u8,
    pub trade_slippage_fee_trade_size_threshold: u8,
    pub imbalance_fee_base: u8,
    pub imbalance_fee_x: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicFeeResult {
    pub arbitrage_fee: u32,
    pub trade_slippage_fee: u32,
    pub imbalance_fee: u32,
    pub swap_dynamic_fee: u32,
    pub total_fee_rate: u32,
}

pub fn calculate_dynamic_fee_rate(inputs: &DynamicFeeInputs) -> Result<DynamicFeeResult> {
    let arbitrage_fee = calculate_arbitrage_fee(
        inputs.p_0,
        inputs.p_index,
        inputs.arbitrage_fee_buffer_ppm,
        inputs.fee_base,
    )?;
    let trade_slippage_fee = calculate_trade_slippage_fee(
        inputs.trade_size,
        inputs.trade_slippage_fee_base,
        inputs.trade_slippage_fee_trade_size_threshold,
    )?;
    let imbalance_fee = calculate_imbalance_fee(
        inputs.quote_value_of_base,
        inputs.quote_balance,
        inputs.imbalance_fee_base,
        inputs.imbalance_fee_x,
        inputs.is_buying_base,
    )?;
    let swap_dynamic_fee = calculate_total_dynamic_fee(arbitrage_fee, trade_slippage_fee, imbalance_fee)?;
    let total_fee_rate = (swap_dynamic_fee as u64)
        .checked_add(inputs.fee_base as u64)
        .ok_or(ErrorCode::CalculateOverflow)?;
    require!(
        total_fee_rate <= FEE_RATE_DENOMINATOR_VALUE as u64,
        ErrorCode::InvalidSwapDynamicFeeParams
    );

    Ok(DynamicFeeResult {
        arbitrage_fee,
        trade_slippage_fee,
        imbalance_fee,
        swap_dynamic_fee,
        total_fee_rate: total_fee_rate as u32,
    })
}

#[inline(always)]
pub fn price_from_sqrt_price_x64(sqrt_price_x64: u128) -> Result<u128> {
    let price_x128 = U256::from(sqrt_price_x64)
        .checked_mul(U256::from(sqrt_price_x64))
        .ok_or(ErrorCode::CalculateOverflow)?;
    let price_x64 = price_x128 >> 64;
    u128::try_from(price_x64).map_err(|_| ErrorCode::CalculateOverflow.into())
}

#[inline(always)]
pub fn quote_amount_from_base(base_amount: u128, price_x64: u128, quote_is_token1: bool) -> Result<u128> {
    require!(price_x64 > 0, ErrorCode::CalculateOverflow);
    if quote_is_token1 {
        Ok(U128::from(base_amount)
            .mul_div_floor(U128::from(price_x64), U128::from(fixed_point_64::Q64))
            .ok_or(ErrorCode::CalculateOverflow)?
            .as_u128())
    } else {
        Ok(U128::from(base_amount)
            .mul_div_floor(U128::from(fixed_point_64::Q64), U128::from(price_x64))
            .ok_or(ErrorCode::CalculateOverflow)?
            .as_u128())
    }
}

/// Normalize `quote_amount` into an integer trade size by dividing by 10^quote_decimals.
pub fn normalize_trade_size(quote_amount: u128, quote_decimals: u8) -> Result<u64> {
    let unit = pow10_u128(quote_decimals as u32)?;
    let trade_size = quote_amount.checked_div(unit).ok_or(ErrorCode::CalculateOverflow)?;
    u64::try_from(trade_size).map_err(|_| ErrorCode::CalculateOverflow.into())
}

fn integer_sqrt_u128(value: u128) -> u128 {
    if value == 0 {
        return 0;
    }
    let mut x = value;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
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
    use crate::states::config::FEE_RATE_DENOMINATOR_VALUE;

    #[test]
    fn test_normalize_trade_size() {
        let trade_size = normalize_trade_size(250_000_000u128, 6).unwrap();
        assert_eq!(trade_size, 250);

        let trade_size = normalize_trade_size(250u128, 0).unwrap();
        assert_eq!(trade_size, 250);
    }

    #[test]
    fn test_calculate_arbitrage_fee() {
        let p_index = fixed_point_64::Q64;
        let p_0 = fixed_point_64::Q64 * 101 / 100; // 1% higher

        let fee = calculate_arbitrage_fee(p_0, p_index, 0, 1_000).unwrap();
        assert_eq!(fee, 9_000);

        let fee = calculate_arbitrage_fee(p_0, p_index, 9_000, 1_000).unwrap();
        assert_eq!(fee, 0);
    }

    #[test]
    fn test_calculate_arbitrage_fee_reverse_direction() {
        let p_index = fixed_point_64::Q64;
        // p_0 < p_index: 1% lower
        let p_0 = fixed_point_64::Q64 * 99 / 100;

        let fee = calculate_arbitrage_fee(p_0, p_index, 0, 1_000).unwrap();
        assert_eq!(fee, 9_001);

        // buffer absorbs the diff
        let fee = calculate_arbitrage_fee(p_0, p_index, 9_000, 1_000).unwrap();
        assert_eq!(fee, 1);
    }

    #[test]
    fn test_calculate_arbitrage_fee_equal_prices() {
        let p = fixed_point_64::Q64;
        let fee = calculate_arbitrage_fee(p, p, 0, 1_000).unwrap();
        assert_eq!(fee, 0);
    }

    #[test]
    fn test_calculate_arbitrage_fee_large_deviation() {
        let p_index = fixed_point_64::Q64;
        // 5% higher
        let p_0 = fixed_point_64::Q64 * 105 / 100;
        let fee = calculate_arbitrage_fee(p_0, p_index, 5_000, 1_000).unwrap();
        // diff_ppm ≈ 50000, fee = 50000 - 5000 - 1000 = 44000
        assert_eq!(fee, 44_000);
    }

    #[test]
    fn test_calculate_trade_slippage_fee() {
        let fee = calculate_trade_slippage_fee(100, 10, 1).unwrap();
        assert_eq!(fee, 0);

        let fee = calculate_trade_slippage_fee(200, 10, 1).unwrap();
        assert_eq!(fee, 100_000);
    }

    #[test]
    fn test_calculate_trade_slippage_fee_base_zero() {
        // base = 0 → fee always 0 regardless of trade size
        let fee = calculate_trade_slippage_fee(500, 0, 1).unwrap();
        assert_eq!(fee, 0);
    }

    #[test]
    fn test_calculate_trade_slippage_fee_threshold_zero() {
        // threshold = 0 → any trade_size > 0 triggers fee
        // delta = 100 * 10^12, sqrt = 10^7
        // fee = ceil(10 * 10_000_000 / 1000) = 100_000
        let fee = calculate_trade_slippage_fee(100, 10, 0).unwrap();
        assert_eq!(fee, 100_000);
    }

    #[test]
    fn test_calculate_trade_slippage_fee_just_above_threshold() {
        // trade_size = 101, threshold = 1 (→100)
        // delta = 1 * 10^12, sqrt = 1_000_000
        // fee = ceil(10 * 1_000_000 / 1000) = 10_000
        let fee = calculate_trade_slippage_fee(101, 10, 1).unwrap();
        assert_eq!(fee, 10_000);
    }

    #[test]
    fn test_calculate_imbalance_fee() {
        let fee = calculate_imbalance_fee(150, 50, 5, 10, false).unwrap();
        assert_eq!(fee, 200_000);

        let fee = calculate_imbalance_fee(150, 50, 5, 10, true).unwrap();
        assert_eq!(fee, 0);

        let fee = calculate_imbalance_fee(100, 100, 5, 10, false).unwrap();
        assert_eq!(fee, 0);
    }

    #[test]
    fn test_calculate_imbalance_fee_reverse() {
        // quote_balance > quote_value_of_base (opposite direction)
        // Same absolute imbalance → same fee
        let fee = calculate_imbalance_fee(50, 150, 5, 10, true).unwrap();
        assert_eq!(fee, 200_000);
    }

    #[test]
    fn test_calculate_imbalance_fee_zero_total() {
        let fee = calculate_imbalance_fee(0, 0, 5, 10, false).unwrap();
        assert_eq!(fee, 0);
    }

    #[test]
    fn test_calculate_imbalance_fee_at_threshold() {
        // imbalance_ppm exactly equals x_ppm → fee = 0
        // total=1000, diff=100, imbalance_ppm=100_000; x=10, x_ppm=100_000
        let fee = calculate_imbalance_fee(550, 450, 5, 10, false).unwrap();
        assert_eq!(fee, 0);
    }

    #[test]
    fn test_calculate_dynamic_fee_rate() {
        let p_index = fixed_point_64::Q64;
        let p_0 = fixed_point_64::Q64 * 101 / 100; // 1% higher

        let inputs = DynamicFeeInputs {
            p_0,
            p_index,
            trade_size: 200,
            quote_value_of_base: 150,
            quote_balance: 50,
            is_buying_base: false,
            fee_base: 1_000,
            arbitrage_fee_buffer_ppm: 0,
            trade_slippage_fee_base: 10,
            trade_slippage_fee_trade_size_threshold: 1,
            imbalance_fee_base: 5,
            imbalance_fee_x: 100,
        };

        let result = calculate_dynamic_fee_rate(&inputs).unwrap();
        assert_eq!(result.arbitrage_fee, 9_000);
        assert_eq!(result.trade_slippage_fee, 100_000);
        assert_eq!(result.imbalance_fee, 0);
        assert_eq!(result.swap_dynamic_fee, 109_000);
        assert_eq!(result.total_fee_rate, 110_000);

        assert!(result.total_fee_rate <= FEE_RATE_DENOMINATOR_VALUE);
    }

    #[test]
    fn test_calculate_dynamic_fee_rate_all_zero() {
        // p_0 == p_index, trade_size below threshold, balanced pool
        let p = fixed_point_64::Q64;
        let inputs = DynamicFeeInputs {
            p_0: p,
            p_index: p,
            trade_size: 0,
            quote_value_of_base: 100,
            quote_balance: 100,
            is_buying_base: false,
            fee_base: 1_000,
            arbitrage_fee_buffer_ppm: 0,
            trade_slippage_fee_base: 10,
            trade_slippage_fee_trade_size_threshold: 1,
            imbalance_fee_base: 5,
            imbalance_fee_x: 10,
        };

        let result = calculate_dynamic_fee_rate(&inputs).unwrap();
        assert_eq!(result.arbitrage_fee, 0);
        assert_eq!(result.trade_slippage_fee, 0);
        assert_eq!(result.imbalance_fee, 0);
        assert_eq!(result.swap_dynamic_fee, 0);
        assert_eq!(result.total_fee_rate, 1_000);
    }

    #[test]
    fn test_calculate_dynamic_fee_rate_exceeds_cap() {
        let p_index = fixed_point_64::Q64;
        // 200% deviation → arbitrage_fee will push total above FEE_RATE_DENOMINATOR_VALUE
        let p_0 = fixed_point_64::Q64 * 3; // 3x price
        let inputs = DynamicFeeInputs {
            p_0,
            p_index,
            trade_size: 0,
            quote_value_of_base: 100,
            quote_balance: 100,
            is_buying_base: false,
            fee_base: 500_000,
            arbitrage_fee_buffer_ppm: 0,
            trade_slippage_fee_base: 0,
            trade_slippage_fee_trade_size_threshold: 1,
            imbalance_fee_base: 0,
            imbalance_fee_x: 10,
        };

        let result = calculate_dynamic_fee_rate(&inputs);
        assert!(result.is_err());
    }

    #[test]
    fn test_calculate_dynamic_fee_rate_all_three_nonzero() {
        let p_index = fixed_point_64::Q64;
        let p_0 = fixed_point_64::Q64 * 105 / 100; // 5% higher

        let inputs = DynamicFeeInputs {
            p_0,
            p_index,
            trade_size: 500,
            quote_value_of_base: 700,
            quote_balance: 300,
            is_buying_base: false,
            fee_base: 1_000,
            arbitrage_fee_buffer_ppm: 5_000,
            trade_slippage_fee_base: 5,
            trade_slippage_fee_trade_size_threshold: 1,
            imbalance_fee_base: 3,
            imbalance_fee_x: 5,
        };

        let result = calculate_dynamic_fee_rate(&inputs).unwrap();
        // arbitrage: diff_ppm≈50000, fee = 50000 - 5000 - 1000 = 44000
        assert_eq!(result.arbitrage_fee, 44_000);
        // trade_slippage: delta=400*10^12, sqrt=20*10^6, fee=ceil(5*20M/1000)=100_000
        assert_eq!(result.trade_slippage_fee, 100_000);
        // imbalance: total=1000, diff=400, ppm=400_000, x_ppm=50_000,
        //   over=350_000, fee=ceil(350_000*3/10)=105_000
        assert_eq!(result.imbalance_fee, 105_000);

        assert_eq!(result.swap_dynamic_fee, 249_000);
        assert_eq!(result.total_fee_rate, 250_000);
        assert!(result.total_fee_rate <= FEE_RATE_DENOMINATOR_VALUE);
    }
}
