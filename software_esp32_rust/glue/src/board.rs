//! WT32-ETH01 v1.2 board wiring of the VdMot controller (C++ `board.h`). The firmware takes these
//! pins from the `Peripherals`; the glue uses the sizes and timings.

/// ESP RX of the STM application UART (`Serial2`): STM PA9.
pub const STM_RX_PIN: u8 = 5;
/// ESP TX of the STM application UART: STM PA10.
pub const STM_TX_PIN: u8 = 17;
/// Baud rate of the STM link (8N1; 8E1 only while flashing).
pub const STM_BAUD: u32 = 115_200;
/// UART RX ring: at least one 1023-character line plus a 256 B bootloader block.
pub const STM_RX_BUFFER_SIZE: usize = 2048;
/// UART TX ring: one bootloader block plus its framing never blocks.
pub const STM_TX_BUFFER_SIZE: usize = 512;

/// STM NRST through an inverting transistor (BC817), jumper X20 fitted: HIGH holds the STM in
/// reset, LOW lets it run. IO15 is a strapping pin with an internal pull-up during the ESP reset:
/// the STM may be held in reset while the ESP boots and runs again as soon as the pin is driven
/// LOW.
pub const STM_RESET_PIN: u8 = 15;
/// Level of [`STM_RESET_PIN`] that holds the STM in reset (HIGH).
pub const STM_RESET_ASSERTED_LEVEL: bool = true;
/// IO14: not connected to STM BOOT0 on the BlackPill boards; driven LOW and otherwise unused.
pub const STM_BOOT0_PIN: u8 = 14;

/// Factory reset pin: GPIO2 (strapping pin) with the pull-up, held LOW for
/// [`FACTORY_RESET_HOLD_MS`] at boot -> factory defaults including the network; once per fitting
/// of the jumper (latch in NVS).
pub const FACTORY_RESET_PIN: u8 = 2;
/// How long the factory pin must stay LOW at boot.
pub const FACTORY_RESET_HOLD_MS: u32 = 5000;
/// Sampling period of the factory pin while it is held.
pub const FACTORY_RESET_SAMPLE_MS: u32 = 50;
/// Settle time of the pull-up before the first sample of the factory pin.
pub const FACTORY_PIN_SETTLE_MS: u32 = 2;

/// LAN8720 PHY address.
pub const ETH_PHY_ADDR: u8 = 1;
/// Oscillator enable of the PHY (Arduino passed it as the PHY power pin).
pub const ETH_PHY_POWER: u8 = 16;
/// MDC of the PHY.
pub const ETH_MDC: u8 = 23;
/// MDIO of the PHY.
pub const ETH_MDIO: u8 = 18;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rx_ring_holds_a_longest_reply_line_and_a_bootloader_block() {
        // kStmMaxLineLen (1023) + CR LF + one 256 B AN3155 block
        const { assert!(STM_RX_BUFFER_SIZE >= 1023 + 2 + 256) };
        // one 256 B block with its address and checksum framing
        const { assert!(STM_TX_BUFFER_SIZE >= 256 + 2 + 5) };
    }

    #[test]
    fn wiring_is_the_one_of_the_cpp_board_header() {
        let pins = [
            STM_RX_PIN,
            STM_TX_PIN,
            STM_RESET_PIN,
            STM_BOOT0_PIN,
            FACTORY_RESET_PIN,
            ETH_PHY_POWER,
            ETH_MDC,
            ETH_MDIO,
        ];
        assert_eq!(pins, [5, 17, 15, 14, 2, 16, 23, 18]);
        assert_eq!(ETH_PHY_ADDR, 1);
        assert_eq!(STM_BAUD, 115_200);
        const { assert!(STM_RESET_ASSERTED_LEVEL) };
        assert_eq!(
            (
                FACTORY_RESET_HOLD_MS,
                FACTORY_RESET_SAMPLE_MS,
                FACTORY_PIN_SETTLE_MS
            ),
            (5000, 50, 2)
        );
    }
}
