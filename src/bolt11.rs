//! The amount of a BOLT11 invoice, read from its human-readable part.
//!
//! The guard never trusts the price a server *says* it charges: the limit is
//! checked against what the invoice itself would take from the wallet.

/// Amount in millisatoshi, or `None` for an invoice with no amount (which
/// the guard refuses: the payer would choose the amount).
pub fn amount_msat(invoice: &str) -> Result<Option<u64>, String> {
    let inv = invoice.trim().to_ascii_lowercase();
    // bech32: the last '1' separates the human-readable part from the data.
    let sep = inv.rfind('1').ok_or("not a BOLT11 invoice: no separator")?;
    let hrp = &inv[..sep];
    let rest = hrp
        .strip_prefix("ln")
        .ok_or("not a BOLT11 invoice: no ln prefix")?;
    // Network prefix is letters (bc, tb, bcrt, tbs); the amount follows.
    let digits_at = rest.find(|c: char| c.is_ascii_digit());
    let Some(at) = digits_at else { return Ok(None) };
    let amount = &rest[at..];
    let (number, multiplier) = match amount.chars().last() {
        Some(c) if c.is_ascii_digit() => (amount, None),
        Some(c) => (&amount[..amount.len() - 1], Some(c)),
        None => return Ok(None),
    };
    let n: u64 = number
        .parse()
        .map_err(|_| "invoice amount is not a number")?;
    // 1 BTC = 100_000_000_000 msat.
    let msat = match multiplier {
        None => n.checked_mul(100_000_000_000),
        Some('m') => n.checked_mul(100_000_000),
        Some('u') => n.checked_mul(100_000),
        Some('n') => n.checked_mul(100),
        Some('p') => {
            if !n.is_multiple_of(10) {
                return Err("invoice amount is finer than a millisatoshi".into());
            }
            Some(n / 10)
        }
        Some(_) => return Err("unknown invoice amount multiplier".into()),
    };
    msat.map(Some)
        .ok_or_else(|| "invoice amount overflows".into())
}

#[cfg(test)]
mod tests {
    use super::amount_msat;

    #[test]
    fn reads_amounts() {
        assert_eq!(amount_msat("lnbc210n1pxyz").unwrap(), Some(21_000));
        assert_eq!(amount_msat("lnbcrt210n1pxyz").unwrap(), Some(21_000));
        assert_eq!(amount_msat("lnbc50u1pxyz").unwrap(), Some(5_000_000));
        assert_eq!(amount_msat("lnbc2m1pxyz").unwrap(), Some(200_000_000));
        assert_eq!(amount_msat("lnbc10p1pxyz").unwrap(), Some(1));
        assert_eq!(amount_msat("LNBC210N1PXYZ").unwrap(), Some(21_000));
    }

    #[test]
    fn no_amount_is_none() {
        assert_eq!(amount_msat("lnbc1pxyz").unwrap(), None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(amount_msat("hello").is_err());
        assert!(amount_msat("lnbc15p1pxyz").is_err());
    }
}
