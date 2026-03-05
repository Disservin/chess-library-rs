use std::fmt;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Not, Shl, Shr};

use crate::coords::{File, Rank, Square};

#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub struct Bitboard(pub u64);

impl Bitboard {
    pub const EMPTY: Bitboard = Bitboard(0);

    #[inline(always)]
    pub const fn from_square(sq: Square) -> Self {
        debug_assert!((sq.0 as usize) < 64);
        Bitboard(1u64 << sq.0)
    }

    #[inline(always)]
    pub const fn from_index(index: u8) -> Self {
        debug_assert!((index as usize) < 64);
        Bitboard(1u64 << index)
    }

    /// Construct a Bitboard with the file's column set.
    pub fn from_file(file: File) -> Self {
        debug_assert!(file != File::NO_FILE);
        Bitboard(0x0101010101010101u64 << (file as u64))
    }

    /// Construct a Bitboard with the rank's row set.
    pub fn from_rank(rank: Rank) -> Self {
        debug_assert!(rank != Rank::NO_RANK);
        Bitboard(0xFFu64 << (8 * rank as u64))
    }

    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Set a bit at `index`.
    #[inline(always)]
    pub fn set(&mut self, index: u8) {
        debug_assert!((index as usize) < 64);
        self.0 |= 1u64 << index;
    }

    /// Clear a bit at `index`.
    #[inline(always)]
    pub fn clear_bit(&mut self, index: u8) {
        debug_assert!((index as usize) < 64);
        self.0 &= !(1u64 << index);
    }

    /// Clear all bits.
    #[inline(always)]
    pub fn clear(&mut self) {
        self.0 = 0;
    }

    /// Check whether bit at `index` is set.
    #[inline(always)]
    pub const fn check(self, index: u8) -> bool {
        self.0 & (1u64 << index) != 0
    }

    /// Index of the least-significant set bit.
    #[inline(always)]
    pub fn lsb(self) -> u8 {
        debug_assert!(self.0 != 0);
        self.0.trailing_zeros() as u8
    }

    /// Index of the most-significant set bit.
    #[inline(always)]
    pub fn msb(self) -> u8 {
        debug_assert!(self.0 != 0);
        63 - self.0.leading_zeros() as u8
    }

    /// Population count (number of set bits).
    #[inline(always)]
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }

    #[inline(always)]
    pub fn pop(&mut self) -> u8 {
        debug_assert!(self.0 != 0);
        let index = self.0.trailing_zeros() as u8;
        self.0 &= self.0 - 1;
        index
    }
}

// ---- operators ----

macro_rules! impl_bitops {
    ($($trait:ident, $method:ident, $assign_trait:ident, $assign_method:ident, $op:tt);* $(;)?) => {$(
        impl $trait for Bitboard {
            type Output = Self;
            #[inline(always)]
            fn $method(self, rhs: Self) -> Self { Bitboard(self.0 $op rhs.0) }
        }
        impl $trait<u64> for Bitboard {
            type Output = Self;
            #[inline(always)]
            fn $method(self, rhs: u64) -> Self { Bitboard(self.0 $op rhs) }
        }
        impl $assign_trait for Bitboard {
            #[inline(always)]
            fn $assign_method(&mut self, rhs: Self) { self.0 = self.0 $op rhs.0; }
        }
    )*};
}

impl_bitops! {
    BitAnd, bitand, BitAndAssign, bitand_assign, &;
    BitOr,  bitor,  BitOrAssign,  bitor_assign,  |;
    BitXor, bitxor, BitXorAssign, bitxor_assign, ^;
}

impl Not for Bitboard {
    type Output = Self;
    #[inline(always)]
    fn not(self) -> Self {
        Bitboard(!self.0)
    }
}

impl Shl<u64> for Bitboard {
    type Output = Self;
    #[inline(always)]
    fn shl(self, rhs: u64) -> Self {
        Bitboard(self.0 << rhs)
    }
}

impl Shr<u64> for Bitboard {
    type Output = Self;
    #[inline(always)]
    fn shr(self, rhs: u64) -> Self {
        Bitboard(self.0 >> rhs)
    }
}

impl From<Bitboard> for bool {
    #[inline(always)]
    fn from(b: Bitboard) -> bool {
        b.0 != 0
    }
}

impl PartialEq<u64> for Bitboard {
    fn eq(&self, other: &u64) -> bool {
        self.0 == *other
    }
}

// ---- display ----

impl fmt::Display for Bitboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for rank in (0..8).rev() {
            for file in 0..8 {
                let c = if self.0 & (1u64 << (rank * 8 + file)) != 0 {
                    '1'
                } else {
                    '0'
                };
                write!(f, "{}", c)?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Bitboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::Square;

    #[test]
    fn lsb() {
        assert_eq!(Bitboard(0x1).lsb(), 0);
        assert_eq!(Bitboard(0x2).lsb(), 1);
        assert_eq!(Bitboard(0x4).lsb(), 2);
    }

    #[test]
    fn msb() {
        assert_eq!(Bitboard(0x8000000000000000).msb(), 63);
        assert_eq!(Bitboard(0x4000000000000000).msb(), 62);
        assert_eq!(Bitboard(0x2000000000000000).msb(), 61);
    }

    #[test]
    fn popcount() {
        assert_eq!(Bitboard(0x1).count(), 1);
        assert_eq!(Bitboard(0x3).count(), 2);
        assert_eq!(Bitboard(0x7).count(), 3);
    }

    #[test]
    fn pop() {
        let mut b = Bitboard(0x1);
        assert_eq!(b.pop(), 0);
        assert_eq!(b, 0u64);

        let mut b = Bitboard(0x3);
        assert_eq!(b.pop(), 0);
        assert_eq!(b, 0x2u64);

        let mut b = Bitboard(0x7);
        assert_eq!(b.pop(), 0);
        assert_eq!(b, 0x6u64);
    }

    #[test]
    fn empty() {
        assert!(Bitboard(0).is_empty());
        assert!(!Bitboard(1).is_empty());
    }

    #[test]
    fn set_bit() {
        let mut b = Bitboard(0);
        b.set(0);
        assert_eq!(b, 0x1u64);
        b.set(1);
        assert_eq!(b, 0x3u64);
        b.set(2);
        assert_eq!(b, 0x7u64);
    }

    #[test]
    fn check_bit() {
        let mut b = Bitboard(0);
        assert!(!b.check(0));
        b.set(0);
        assert!(b.check(0));
        b.set(1);
        assert!(b.check(1));
    }

    #[test]
    fn clear_bit() {
        let mut b = Bitboard(0);
        b.set(0);
        b.clear_bit(0);
        assert_eq!(b, 0u64);
        b.set(1);
        b.clear_bit(1);
        assert_eq!(b, 0u64);
    }

    #[test]
    fn clear_all() {
        let mut b = Bitboard(0);
        b.set(0);
        b.clear();
        assert_eq!(b, 0u64);
    }

    #[test]
    fn from_index() {
        assert_eq!(Bitboard::from_index(0), 0x1u64);
        assert_eq!(Bitboard::from_index(1), 0x2u64);
        assert_eq!(Bitboard::from_index(2), 0x4u64);
    }

    #[test]
    fn ops() {
        assert_eq!(Bitboard(0x3) & Bitboard(0x1), 0x1u64);
        assert_eq!(Bitboard(0x1) | Bitboard(0x2), 0x3u64);
        assert_eq!(Bitboard(0x3) ^ Bitboard(0x3), 0u64);
        assert_eq!(!Bitboard(0), 0xffffffffffffffffu64);
        assert_eq!(Bitboard(0x1) << 1u64, 0x2u64);
        assert_eq!(Bitboard(0x2) >> 1u64, 0x1u64);

        let mut b = Bitboard(0x3);
        b &= Bitboard(0x1);
        assert_eq!(b, 0x1u64);
        b |= Bitboard(0x2);
        assert_eq!(b, 0x3u64);
        b ^= Bitboard(0x3);
        assert_eq!(b, 0u64);
    }

    #[test]
    fn display_bitboard() {
        let s = format!("{}", Bitboard::from_square(Square::SQ_A1));
        assert!(s.ends_with("10000000\n"));
    }
}
