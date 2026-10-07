//
// The 24LC64 of the VdMot controller (8 KiB, 32-byte pages, two address bytes) as an I2C slave
// for the Renode tests of the Rust STM32 images (docs/rust/GLUE-DESIGN-STM.md §5.7). Compiled by
// Renode at run time (include @VdmEeprom.cs). Renode's STM32F4_I2C hands over the bytes of a
// write transfer in one Write call at its end and never calls FinishTransmission, so one call is
// one transfer: the two address bytes, then the data bytes, written from the address on with the
// pointer wrapping inside the 32-byte page; a transfer without bytes (the address probe of the
// ready polling) changes nothing. A read transfer reads from the pointer on, across the whole
// chip. The content survives a machine reset (an EEPROM), erased (0xFF) at creation.
//

using System;
using System.Text;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.I2C;

namespace Antmicro.Renode.Peripherals.I2C
{
    public class VdmEeprom : II2CPeripheral
    {
        public VdmEeprom()
        {
            memory = new byte[Size];
            Erase();
        }

        public void Write(byte[] data)
        {
            if(data.Length < 2)
            {
                return;
            }
            pointer = ((data[0] << 8) | data[1]) & (Size - 1);
            for(var i = 2; i < data.Length; i++)
            {
                memory[pointer] = data[i];
                Writes++;
                pointer = (pointer & ~(PageSize - 1)) | ((pointer + 1) & (PageSize - 1));
            }
        }

        // STM32F4_I2C asks once per read transfer (Read() with the default count) and gives the
        // master the bytes returned: a burst longer than the firmware's chunks of 30 bytes; the
        // master's NACK and STOP end the transfer and the rest is dropped
        public byte[] Read(int count = 1)
        {
            count = Math.Max(count, ReadBurst);
            var result = new byte[count];
            for(var i = 0; i < count; i++)
            {
                result[i] = memory[pointer];
                pointer = (pointer + 1) & (Size - 1);
            }
            return result;
        }

        public void FinishTransmission()
        {
        }

        public void Reset()
        {
            // an EEPROM keeps its content
        }

        public void Erase()
        {
            for(var i = 0; i < Size; i++)
            {
                memory[i] = 0xFF;
            }
        }

        public byte GetByte(int address)
        {
            return memory[address & (Size - 1)];
        }

        public void SetByte(int address, byte value)
        {
            memory[address & (Size - 1)] = value;
        }

        // the bytes of a hex string from address on (a row of a golden transcript)
        public void LoadHex(int address, string hex)
        {
            for(var i = 0; i + 1 < hex.Length; i += 2)
            {
                memory[(address + i / 2) & (Size - 1)] = Convert.ToByte(hex.Substring(i, 2), 16);
            }
        }

        // the bytes [address, address + count) as hex, for the tests
        public string Hex(int address, int count)
        {
            var s = new StringBuilder();
            for(var i = 0; i < count; i++)
            {
                s.Append(memory[(address + i) & (Size - 1)].ToString("x2"));
            }
            return s.ToString();
        }

        // data bytes written since the creation
        public long Writes { get; private set; }

        private int pointer;
        private readonly byte[] memory;

        private const int Size = 8192;
        private const int PageSize = 32;
        private const int ReadBurst = 32;
    }
}
