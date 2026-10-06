//! The Thumb-2 instructions that reach other code or data (ARMv7-M ARM, A7.7): branches,
//! calls, literal loads, ADR and MOVW/MOVT pairs. Everything else is `Op::Other`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// BL
    Call(u32),
    /// B (also a tail call), B<cond>, CBZ, CBNZ
    Branch(u32),
    /// a load from a PC-relative address (LDR/LDRB/LDRH/LDRD/VLDR literal); `word` for a
    /// 32-bit load whose value may be an address
    Literal { addr: u32, word: bool },
    /// ADR: an address computed from the PC
    Adr(u32),
    MovW { rd: u8, imm: u16 },
    MovT { rd: u8, imm: u16 },
    /// TBB/TBH: a jump table inside the function
    Table,
    /// BX LR, POP {.., PC}
    Return,
    /// BX Rm, BLX Rm, MOV PC, Rm: a target the decoder cannot know
    Indirect,
    Other,
}

/// Instruction length from its first halfword.
pub fn len(hw1: u16) -> u32 {
    if matches!(hw1 >> 11, 0b11101..=0b11111) {
        4
    } else {
        2
    }
}

fn sign_extend(v: u32, bits: u32) -> u32 {
    let shift = 32 - bits;
    (((v << shift) as i32) >> shift) as u32
}

fn align4(pc: u32) -> u32 {
    pc.wrapping_add(4) & !3
}

/// Decodes the instruction at `pc` (`hw2` is ignored for 16-bit instructions).
pub fn decode(hw1: u16, hw2: u16, pc: u32) -> Op {
    let h1 = u32::from(hw1);
    let h2 = u32::from(hw2);
    if len(hw1) == 2 {
        if h1 & 0xF800 == 0x4800 {
            return Op::Literal {
                addr: align4(pc).wrapping_add((h1 & 0xFF) << 2),
                word: true,
            };
        }
        if h1 & 0xF800 == 0xA000 {
            return Op::Adr(align4(pc).wrapping_add((h1 & 0xFF) << 2));
        }
        if h1 & 0xF000 == 0xD000 && (h1 >> 8) & 0xF < 0xE {
            return Op::Branch(pc.wrapping_add(4).wrapping_add(sign_extend((h1 & 0xFF) << 1, 9)));
        }
        if h1 & 0xF800 == 0xE000 {
            return Op::Branch(pc.wrapping_add(4).wrapping_add(sign_extend((h1 & 0x7FF) << 1, 12)));
        }
        if h1 & 0xF500 == 0xB100 {
            let imm = (((h1 >> 9) & 1) << 6) | (((h1 >> 3) & 0x1F) << 1);
            return Op::Branch(pc.wrapping_add(4).wrapping_add(imm));
        }
        if h1 & 0xFF87 == 0x4700 {
            return if (h1 >> 3) & 0xF == 14 { Op::Return } else { Op::Indirect };
        }
        if h1 & 0xFF87 == 0x4780 || h1 & 0xFF87 == 0x4687 {
            return Op::Indirect;
        }
        if h1 & 0xFF00 == 0xBD00 {
            return Op::Return;
        }
        return Op::Other;
    }

    // B.W, B<cond>.W, BL
    if h1 & 0xF800 == 0xF000 && h2 & 0x8000 != 0 {
        let s = (h1 >> 10) & 1;
        let j1 = (h2 >> 13) & 1;
        let j2 = (h2 >> 11) & 1;
        match h2 & 0xD000 {
            0xD000 | 0x9000 => {
                let i1 = !(j1 ^ s) & 1;
                let i2 = !(j2 ^ s) & 1;
                let imm = (s << 24) | (i1 << 23) | (i2 << 22) | ((h1 & 0x3FF) << 12) | ((h2 & 0x7FF) << 1);
                let target = pc.wrapping_add(4).wrapping_add(sign_extend(imm, 25));
                return if h2 & 0xD000 == 0xD000 {
                    Op::Call(target)
                } else {
                    Op::Branch(target)
                };
            }
            0x8000 if (h1 >> 6) & 0xF < 0xE => {
                let imm = (s << 20) | (j2 << 19) | (j1 << 18) | ((h1 & 0x3F) << 12) | ((h2 & 0x7FF) << 1);
                return Op::Branch(pc.wrapping_add(4).wrapping_add(sign_extend(imm, 21)));
            }
            _ => return Op::Other,
        }
    }
    // LDR{,B,H,SB,SH} (literal), PLD/PLI (literal)
    if h1 & 0xFE1F == 0xF81F {
        let imm = h2 & 0xFFF;
        let base = align4(pc);
        let addr = if h1 & 0x80 != 0 { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
        // size bits 6:5 = 10: a word
        return Op::Literal {
            addr,
            word: (h1 >> 5) & 0b11 == 0b10 && h1 & 0x100 == 0,
        };
    }
    // LDRD (literal; P = W = 0 is the table branch and exclusive group), VLDR (literal)
    if (h1 & 0xFE5F == 0xE85F && h1 & 0x0120 != 0) || h1 & 0xFF3F == 0xED1F {
        let imm = (h2 & 0xFF) << 2;
        let base = align4(pc);
        let addr = if h1 & 0x80 != 0 { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
        return Op::Literal { addr, word: false };
    }
    // ADR (T3 add, T2 sub)
    if (h1 & 0xFBFF == 0xF20F || h1 & 0xFBFF == 0xF2AF) && h2 & 0x8000 == 0 {
        let imm = (((h1 >> 10) & 1) << 11) | (((h2 >> 12) & 0x7) << 8) | (h2 & 0xFF);
        let base = align4(pc);
        return Op::Adr(if h1 & 0x00A0 == 0x00A0 {
            base.wrapping_sub(imm)
        } else {
            base.wrapping_add(imm)
        });
    }
    // MOVW, MOVT
    if (h1 & 0xFBF0 == 0xF240 || h1 & 0xFBF0 == 0xF2C0) && h2 & 0x8000 == 0 {
        let imm = ((h1 & 0xF) << 12) | (((h1 >> 10) & 1) << 11) | (((h2 >> 12) & 0x7) << 8) | (h2 & 0xFF);
        let rd = ((h2 >> 8) & 0xF) as u8;
        return if h1 & 0xFBF0 == 0xF240 {
            Op::MovW { rd, imm: imm as u16 }
        } else {
            Op::MovT { rd, imm: imm as u16 }
        };
    }
    // TBB, TBH
    if h1 & 0xFFF0 == 0xE8D0 && h2 & 0xFFE0 == 0xF000 {
        return Op::Table;
    }
    // POP.W {.., PC} (LDMIA SP!), LDR.W PC, [SP], #4
    if (h1 == 0xE8BD && h2 & 0x8000 != 0) || (h1 == 0xF85D && h2 == 0xFB04) {
        return Op::Return;
    }
    Op::Other
}

#[cfg(test)]
mod tests;
