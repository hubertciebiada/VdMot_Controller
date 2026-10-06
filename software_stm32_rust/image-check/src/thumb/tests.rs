// Encodings taken from llvm-objdump of the F401 C2 image and from the ARMv7-M ARM.
use super::*;

#[test]
fn calls_and_branches() {
    // bl 0x80002d4 at 0x8000240 (Reset -> __pre_init)
    assert_eq!(decode(0xF000, 0xF848, 0x0800_0240), Op::Call(0x0800_02D4));
    // bl backwards: 0x08001000 -> 0x08000300
    assert_eq!(decode(0xF7FF, 0xF97E, 0x0800_1000), Op::Call(0x0800_0300));
    // b.w 0x80002b0 at 0x80002a4 (HardFault trampoline)
    assert_eq!(decode(0xF000, 0xB804, 0x0800_02A4), Op::Branch(0x0800_02B0));
    // bne.w +0x100
    assert_eq!(decode(0xF040, 0x8080, 0x0800_1000), Op::Branch(0x0800_1104));
    // beq +2, b -10 (16 bit)
    assert_eq!(decode(0xD001, 0, 0x0800_024C), Op::Branch(0x0800_0252));
    assert_eq!(decode(0xE7FB, 0, 0x0800_0250), Op::Branch(0x0800_024A));
    // cbz r0, +6
    assert_eq!(decode(0xB118, 0, 0x0800_1000), Op::Branch(0x0800_100A));
    // udf #0 is not a branch
    assert_eq!(decode(0xDE00, 0, 0x0800_1000), Op::Other);
}

#[test]
fn literals_and_addresses() {
    // ldr r0, [pc, #0x38] at 0x8000244
    assert_eq!(
        decode(0x480E, 0, 0x0800_0244),
        Op::Literal {
            addr: 0x0800_0280,
            word: true
        }
    );
    // ldr.w r1, [pc, #-8] at 0x08000400; ldrb.w r1, [pc, #8]
    assert_eq!(
        decode(0xF85F, 0x1008, 0x0800_0400),
        Op::Literal {
            addr: 0x0800_03FC,
            word: true
        }
    );
    assert_eq!(
        decode(0xF89F, 0x1008, 0x0800_0400),
        Op::Literal {
            addr: 0x0800_040C,
            word: false
        }
    );
    // adr r0, #8 at 0x08000402
    assert_eq!(decode(0xA002, 0, 0x0800_0402), Op::Adr(0x0800_040C));
    // adr.w r0, #0x123 (add) and #-0x10 (sub)
    assert_eq!(decode(0xF20F, 0x1023, 0x0800_0400), Op::Adr(0x0800_0527));
    assert_eq!(decode(0xF2AF, 0x0010, 0x0800_0400), Op::Adr(0x0800_03F4));
    // movw r0, #0x1234; movt r0, #0x0800
    assert_eq!(decode(0xF241, 0x2034, 0), Op::MovW { rd: 0, imm: 0x1234 });
    assert_eq!(decode(0xF6C0, 0x0000, 0), Op::MovT { rd: 0, imm: 0x0800 });
    assert_eq!(
        decode(0xF6C0, 0x0C00, 0),
        Op::MovT {
            rd: 12,
            imm: 0x0800
        }
    );
}

#[test]
fn returns_and_indirect_jumps() {
    assert_eq!(decode(0x4770, 0, 0), Op::Return); // bx lr
    assert_eq!(decode(0xBD80, 0, 0), Op::Return); // pop {r7, pc}
    assert_eq!(decode(0xE8BD, 0x8FF0, 0), Op::Return); // pop.w {r4-r11, pc}
    assert_eq!(decode(0x4708, 0, 0), Op::Indirect); // bx r1
    assert_eq!(decode(0x4798, 0, 0), Op::Indirect); // blx r3
    assert_eq!(decode(0xE8DF, 0xF000, 0), Op::Table); // tbb [pc, r0]
}

#[test]
fn instruction_length() {
    assert_eq!(len(0x4770), 2);
    assert_eq!(len(0xE7FB), 2);
    assert_eq!(len(0xE8BD), 4);
    assert_eq!(len(0xF000), 4);
    assert_eq!(len(0xF85F), 4);
}
