use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub numerator: i128,
    pub denominator: i128,
}

pub fn offset_us(qpc100ns: u64, accepted_epoch100ns: u64) -> Rational {
    Rational {
        numerator: i128::from(qpc100ns) - i128::from(accepted_epoch100ns),
        denominator: 10,
    }
}

pub fn raw_cutoff_100ns(ticks: u64, frequency: u64) -> Result<Rational> {
    if frequency == 0 {
        return Err("QPC frequency is zero".into());
    }
    Ok(Rational {
        numerator: i128::from(ticks) * 10_000_000,
        denominator: i128::from(frequency),
    })
}

pub fn interpolate(frame: u64, a: (u64, u64), b: (u64, u64)) -> Result<Rational> {
    if b.0 <= a.0 || b.1 <= a.1 || frame < a.0 || frame > b.0 {
        return Err("Untrusted or extrapolated clock interval".into());
    }
    let df = i128::from(b.0 - a.0);
    let dq = i128::from(b.1 - a.1);
    let numerator = i128::from(a.1).checked_mul(df).and_then(|n| {
        i128::from(frame - a.0).checked_mul(dq).and_then(|m| n.checked_add(m))
    }).ok_or("Clock interpolation overflow")?;
    Ok(Rational { numerator, denominator: df })
}

pub fn drift_ppm(a: (u64, u64), b: (u64, u64), rate: u32) -> Result<Rational> {
    if b.0 <= a.0 || b.1 <= a.1 || !(8000..=192000).contains(&rate) {
        return Err("Invalid drift anchors".into());
    }
    let denominator = i128::from(b.0 - a.0) * 10_000_000;
    let delta = i128::from(b.1 - a.1) * i128::from(rate) - denominator;
    Ok(Rational {
        numerator: delta.checked_mul(1_000_000).ok_or("Drift overflow")?,
        denominator,
    })
}
