//! Arduino `Print` of the STM32 core (framework-arduinoststm32 `Print.cpp`) on its 32-bit
//! target: the protocol v1 replies to the ESP and the debug lines on USART6 are written with it,
//! byte for byte as the C++ `print`/`println` calls write them.

use crate::hal::Serial;

/// `DEC`
pub const DEC: u8 = 10;
/// `HEX`
pub const HEX: u8 = 16;

/// A byte sink with the formatting of Arduino `Print`; every [`Serial`] is one. The integer
/// forms take the C++ overloads by their 32-bit types: `print(long, base)` and `int` are
/// [`print_signed`](Print::print_signed), `unsigned long`, `unsigned int` and `unsigned char`
/// are [`print_unsigned`](Print::print_unsigned).
pub trait Print {
    /// `write(buffer, size)`
    fn write_bytes(&mut self, bytes: &[u8]);

    /// `print(const char*)`
    fn print(&mut self, text: &[u8]) {
        self.write_bytes(text);
    }

    /// `print(char)`
    fn print_char(&mut self, c: u8) {
        self.write_bytes(&[c]);
    }

    /// `print(unsigned long n, int base)`: base 0 writes the low byte of n, a base below 2
    /// prints decimal, digits above 9 are upper case.
    fn print_unsigned(&mut self, n: u32, base: u8) {
        if base == 0 {
            self.write_bytes(&[n as u8]);
        } else {
            print_number(self, n, base);
        }
    }

    /// `print(long n, int base)`: base 10 prints the sign, every other base the unsigned two's
    /// complement (`print(-1, HEX)` is "FFFFFFFF"), base 0 the low byte.
    fn print_signed(&mut self, n: i32, base: u8) {
        if base == 10 && n < 0 {
            self.print_char(b'-');
            print_number(self, n.unsigned_abs(), 10);
        } else {
            self.print_unsigned(n as u32, base);
        }
    }

    /// `print(double n, int digits)`
    fn print_f64(&mut self, number: f64, digits: u8) {
        if number.is_nan() {
            return self.print(b"nan");
        }
        if number.is_infinite() {
            return self.print(b"inf");
        }
        // constant determined empirically (the core's comment)
        if !(-4_294_967_040.0..=4_294_967_040.0).contains(&number) {
            return self.print(b"ovf");
        }
        let mut number = number;
        if number < 0.0 {
            self.print_char(b'-');
            number = -number;
        }
        // round so that print(1.999, 2) prints "2.00"
        let mut rounding = 0.5f64;
        for _ in 0..digits {
            rounding /= 10.0;
        }
        number += rounding;
        let int_part = number as u32;
        let mut remainder = number - f64::from(int_part);
        self.print_unsigned(int_part, DEC);
        if digits > 0 {
            self.print_char(b'.');
        }
        for _ in 0..digits {
            remainder *= 10.0;
            let to_print = remainder as u32;
            self.print_unsigned(to_print, DEC);
            remainder -= f64::from(to_print);
        }
    }

    /// `print(float n, int digits)`: the core's template computes in float, every `x op= 10.0`
    /// in double rounded back to float.
    fn print_f32(&mut self, number: f32, digits: u8) {
        if number.is_nan() {
            return self.print(b"nan");
        }
        if number.is_infinite() {
            return self.print(b"inf");
        }
        if !(-4_294_967_040.0..=4_294_967_040.0).contains(&f64::from(number)) {
            return self.print(b"ovf");
        }
        let mut number = number;
        if number < 0.0 {
            self.print_char(b'-');
            number = -number;
        }
        let mut rounding = 0.5f32;
        for _ in 0..digits {
            rounding = (f64::from(rounding) / 10.0) as f32;
        }
        number += rounding;
        let int_part = number as u32;
        let mut remainder = number - int_part as f32;
        self.print_unsigned(int_part, DEC);
        if digits > 0 {
            self.print_char(b'.');
        }
        for _ in 0..digits {
            remainder = (f64::from(remainder) * 10.0) as f32;
            let to_print = remainder as u32;
            self.print_unsigned(to_print, DEC);
            remainder -= to_print as f32;
        }
    }

    /// `println()`: CR LF
    fn println(&mut self) {
        self.write_bytes(b"\r\n");
    }

    fn println_text(&mut self, text: &[u8]) {
        self.print(text);
        self.println();
    }

    fn println_char(&mut self, c: u8) {
        self.print_char(c);
        self.println();
    }

    fn println_unsigned(&mut self, n: u32, base: u8) {
        self.print_unsigned(n, base);
        self.println();
    }

    fn println_signed(&mut self, n: i32, base: u8) {
        self.print_signed(n, base);
        self.println();
    }
}

impl<S: Serial + ?Sized> Print for S {
    fn write_bytes(&mut self, bytes: &[u8]) {
        self.write(bytes);
    }
}

/// `printNumber`: the digits of n in base (2..255; below 2 prints decimal), the last digit
/// computed first.
fn print_number<P: Print + ?Sized>(p: &mut P, n: u32, base: u8) {
    let base = if base < 2 { 10 } else { u32::from(base) };
    let mut buf = [0u8; 32];
    let mut i = buf.len();
    let mut n = n;
    loop {
        let m = n;
        n /= base;
        let c = (m - base * n) as u8;
        i -= 1;
        buf[i] = if c < 10 {
            c + b'0'
        } else {
            c.wrapping_add(b'A' - 10)
        };
        if n == 0 {
            break;
        }
    }
    p.write_bytes(&buf[i..]);
}

#[cfg(test)]
mod tests;
