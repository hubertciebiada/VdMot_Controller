//
// The ST ROM bootloader (AN3155 over USART1) for Renode scenario E11 (docs/rust/GLUE-DESIGN-STM.md
// §5.7): the program the chip runs after the boot stage's jump to 0x1FFF0000. The platform has
// only a stand-in there ("b ."), so while the CPU is in the system memory this model does that
// program's work on the registers of USART1: it sets the USART to 8E1 as the ROM does, reads
// SR/DR every 50 us, answers the commands of the ESP flasher (sync 0x7F, GET, GET ID, Extended
// Erase with a sector list, Write Memory, Read Memory) and erases and writes the flash memory.
// A reset ends the session. Every erase, write and read is logged for the order check of D9.
// Compiled by Renode at run time (include @VdmRom.cs).
//

using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class VdmRom : IDoubleWordPeripheral, IKnownSize
    {
        public VdmRom(IMachine machine, int pid = 0x423, int flashKiB = 256)
        {
            this.machine = machine;
            this.pid = pid;
            sectors = flashKiB > 256 ? 8 : 6;
            buf = new List<byte>();
            ops = new List<string>();
            poll = new LimitTimer(machine.ClockSource, 20000, this, "rom_poll", limit: 1, direction: Direction.Ascending, enabled: true, eventEnabled: true, autoUpdate: true);
            poll.LimitReached += OnPoll;
        }

        public long Size => 0x4;

        public uint ReadDoubleWord(long offset)
        {
            return 0;
        }

        public void WriteDoubleWord(long offset, uint value)
        {
        }

        public void Reset()
        {
            active = false;
            state = St.Cmd;
            buf.Clear();
        }

        // ROM sessions so far
        public int Sessions { get; private set; }

        // the erases, writes and reads, runs of consecutive blocks merged ("W 08004000..0800C100 x33")
        public string Operations()
        {
            var s = new StringBuilder();
            var i = 0;
            while(i < ops.Count)
            {
                var op = ops[i];
                if(op[0] == 'W' || op[0] == 'R')
                {
                    var first = Convert.ToUInt32(op.Substring(2), 16);
                    var last = first;
                    var j = i + 1;
                    while(j < ops.Count && ops[j][0] == op[0] && Convert.ToUInt32(ops[j].Substring(2), 16) == last + BlockSize)
                    {
                        last += BlockSize;
                        j++;
                    }
                    s.AppendFormat("{0} {1:X8}..{2:X8} x{3}; ", op[0], first, last, j - i);
                    i = j;
                    continue;
                }
                s.Append(op).Append("; ");
                i++;
            }
            return s.ToString().TrimEnd(' ', ';');
        }

        private void OnPoll()
        {
            var bus = machine.SystemBus;
            if(!active)
            {
                var cpu = bus.GetCPUs().FirstOrDefault();
                if(cpu == null)
                {
                    return;
                }
                var pc = cpu.PC.RawValue;
                if(pc < RomBase || pc >= RomEnd)
                {
                    return;
                }
                active = true;
                Sessions++;
                ops.Add("session " + Sessions);
                // the ROM sets USART1 up itself: 8E1, receiver and transmitter on
                bus.WriteDoubleWord(UsartCr1, 0x340C);
                state = St.Cmd;
                buf.Clear();
            }
            while((bus.ReadDoubleWord(UsartSr) & SrRxne) != 0)
            {
                Feed((byte)bus.ReadDoubleWord(UsartDr));
            }
        }

        private void Feed(byte b)
        {
            switch(state)
            {
            case St.Cmd:
                if(buf.Count == 0 && b == Sync)
                {
                    Send(Ack);
                    return;
                }
                buf.Add(b);
                if(buf.Count < 2)
                {
                    return;
                }
                var cmd = buf[0];
                var ok = buf[1] == (byte)~cmd;
                buf.Clear();
                if(!ok)
                {
                    Send(Nack);
                    return;
                }
                Command(cmd);
                return;
            case St.EraseHead:
                buf.Add(b);
                if(buf.Count < 2)
                {
                    return;
                }
                var n = (buf[0] << 8) | buf[1];
                if(n >= 0xFFF0 || n >= sectors)
                {
                    // mass and bank erase are not used by the flasher
                    Fail();
                    return;
                }
                need = 2 + (n + 1) * 2 + 1;
                state = St.EraseBody;
                return;
            case St.EraseBody:
                buf.Add(b);
                if(buf.Count < need)
                {
                    return;
                }
                if(!Checksum(buf))
                {
                    Fail();
                    return;
                }
                var list = new List<int>();
                for(var i = 2; i + 1 < buf.Count - 1; i += 2)
                {
                    list.Add((buf[i] << 8) | buf[i + 1]);
                }
                if(list.Any(s => s >= sectors))
                {
                    Fail();
                    return;
                }
                foreach(var s in list)
                {
                    var fill = new byte[SectorSize[s]];
                    for(var k = 0; k < fill.Length; k++)
                    {
                        fill[k] = 0xFF;
                    }
                    machine.SystemBus.WriteBytes(fill, FlashBase + SectorStart(s));
                }
                ops.Add("E " + string.Join(" ", list));
                Done();
                return;
            case St.WriteAddr:
            case St.ReadAddr:
                buf.Add(b);
                if(buf.Count < 5)
                {
                    return;
                }
                if(!Checksum(buf))
                {
                    Fail();
                    return;
                }
                address = ((uint)buf[0] << 24) | ((uint)buf[1] << 16) | ((uint)buf[2] << 8) | buf[3];
                if(address < FlashBase || address >= FlashBase + FlashEnd())
                {
                    Fail();
                    return;
                }
                Send(Ack);
                buf.Clear();
                state = state == St.WriteAddr ? St.WriteData : St.ReadLen;
                return;
            case St.WriteData:
                buf.Add(b);
                if(buf.Count < 1 + buf[0] + 1 + 1)
                {
                    return;
                }
                if(!Checksum(buf))
                {
                    Fail();
                    return;
                }
                machine.SystemBus.WriteBytes(buf.Skip(1).Take(buf[0] + 1).ToArray(), address);
                ops.Add(string.Format("W {0:X8}", address));
                Done();
                return;
            case St.ReadLen:
                buf.Add(b);
                if(buf.Count < 2)
                {
                    return;
                }
                if(buf[1] != (byte)~buf[0])
                {
                    Fail();
                    return;
                }
                Send(Ack);
                Send(machine.SystemBus.ReadBytes(address, buf[0] + 1));
                ops.Add(string.Format("R {0:X8}", address));
                buf.Clear();
                state = St.Cmd;
                return;
            }
        }

        private void Command(byte cmd)
        {
            switch(cmd)
            {
            case 0x00: // GET: version 3.1 and the commands
                Send(Ack);
                Send(new byte[] { 11, 0x31, 0x00, 0x01, 0x02, 0x11, 0x21, 0x31, 0x44, 0x63, 0x73, 0x82, 0x92 });
                Send(Ack);
                return;
            case 0x01: // GET VERSION
                Send(Ack);
                Send(new byte[] { 0x31, 0x00, 0x00 });
                Send(Ack);
                return;
            case 0x02: // GET ID
                Send(Ack);
                Send(new byte[] { 1, (byte)(pid >> 8), (byte)pid });
                Send(Ack);
                return;
            case 0x11:
                Send(Ack);
                state = St.ReadAddr;
                return;
            case 0x31:
                Send(Ack);
                state = St.WriteAddr;
                return;
            case 0x44:
                Send(Ack);
                state = St.EraseHead;
                return;
            default:
                Send(Nack);
                return;
            }
        }

        // the last byte is the XOR of the ones before (one byte alone: its complement)
        private static bool Checksum(List<byte> bytes)
        {
            byte x = 0;
            for(var i = 0; i < bytes.Count - 1; i++)
            {
                x ^= bytes[i];
            }
            return x == bytes[bytes.Count - 1];
        }

        private void Done()
        {
            Send(Ack);
            buf.Clear();
            state = St.Cmd;
        }

        private void Fail()
        {
            ops.Add("NACK");
            Send(Nack);
            buf.Clear();
            state = St.Cmd;
        }

        private void Send(byte b)
        {
            machine.SystemBus.WriteDoubleWord(UsartDr, b);
        }

        private void Send(byte[] bytes)
        {
            foreach(var b in bytes)
            {
                Send(b);
            }
        }

        private uint FlashEnd()
        {
            return SectorStart(sectors);
        }

        private static uint SectorStart(int s)
        {
            uint start = 0;
            for(var i = 0; i < s; i++)
            {
                start += (uint)SectorSize[i];
            }
            return start;
        }

        private enum St
        {
            Cmd,
            EraseHead,
            EraseBody,
            WriteAddr,
            WriteData,
            ReadAddr,
            ReadLen
        }

        private readonly IMachine machine;
        private readonly int pid;
        private readonly int sectors;
        private readonly List<byte> buf;
        private readonly List<string> ops;
        private readonly LimitTimer poll;
        private bool active;
        private St state;
        private int need;
        private uint address;

        private const byte Ack = 0x79;
        private const byte Nack = 0x1F;
        private const byte Sync = 0x7F;
        private const uint BlockSize = 256;
        private const ulong RomBase = 0x1FFF0000;
        private const ulong RomEnd = 0x1FFF7800;
        private const uint FlashBase = 0x08000000;
        private const ulong UsartSr = 0x40011000;
        private const ulong UsartDr = 0x40011004;
        private const ulong UsartCr1 = 0x4001100C;
        private const uint SrRxne = 0x20;
        private static readonly int[] SectorSize = { 0x4000, 0x4000, 0x4000, 0x4000, 0x10000, 0x20000, 0x20000, 0x20000 };
    }
}
