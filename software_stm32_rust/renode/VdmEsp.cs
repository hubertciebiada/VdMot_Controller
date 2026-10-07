//
// The ESP32 of the VdMot controller for Renode scenario E11 (docs/rust/GLUE-DESIGN-STM.md §5.7):
// its end of USART1 (an IUART on a UART hub with sysbus.usart1) and of the STM's NRST line. The
// flasher itself is the Rust ESP flasher in a host program (tools/rust/stm/e11, vdm-e11) that
// this model starts and drives in lock-step, once per millisecond of virtual time: it hands over
// the bytes the STM sent, takes the bytes to send and the NRST level, and resets the machine
// when NRST is released (the STM runs on while NRST is held: a reset pulse is one reset).
// Compiled by Renode at run time (include @VdmEsp.cs).
//

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Text;
using Antmicro.Migrant;
using Antmicro.Migrant.Hooks;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.UART
{
    public class VdmEsp : IUART, IDoubleWordPeripheral, IKnownSize
    {
        public VdmEsp(IMachine machine)
        {
            this.machine = machine;
            rx = new List<byte>();
            lines = new List<string>();
            results = new List<string>();
            tick = new LimitTimer(machine.ClockSource, 1000, this, "esp_ms", limit: 1, direction: Direction.Ascending, enabled: false, eventEnabled: true, autoUpdate: true);
            tick.LimitReached += OnTick;
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
            // the ESP is not reset with the STM
        }

        // ---- IUART: the STM's TX arrives here, CharReceived goes to the STM's RX

        public void WriteChar(byte value)
        {
            lock(rx)
            {
                rx.Add(value);
            }
        }

        public event Action<byte> CharReceived;

        public uint BaudRate => 115200;

        public Bits StopBits => Bits.One;

        public Parity ParityBit => Parity.None;

        // ---- the tests (monitor: sysbus.esp <Method> <args>)

        // starts the flasher program; it runs until all its jobs ended
        public void Start(string program, string arguments)
        {
            var info = new ProcessStartInfo(program, arguments)
            {
                RedirectStandardInput = true,
                RedirectStandardOutput = true,
                UseShellExecute = false
            };
            process = Process.Start(info);
            tick.Enabled = true;
        }

        // "ok" or "fail" once every job ended, "" before
        public string Result { get; private set; } = "";

        // the job results (R lines)
        public string Results => string.Join("\n", results);

        // everything the program reported (L, R, END lines)
        public string Report => string.Join("\n", lines);

        // NRST releases (machine resets) so far
        public int Resets { get; private set; }

        private void OnTick()
        {
            if(process == null || process.HasExited)
            {
                return;
            }
            var now = (ulong)machine.ElapsedVirtualTime.TimeElapsed.TotalMilliseconds;
            string received;
            lock(rx)
            {
                received = Hex(rx);
                rx.Clear();
            }
            process.StandardInput.WriteLine("T " + now + " " + received);
            process.StandardInput.Flush();
            while(true)
            {
                var line = process.StandardOutput.ReadLine();
                if(line == null)
                {
                    this.Log(LogLevel.Error, "vdm-e11 ended without a reply");
                    tick.Enabled = false;
                    return;
                }
                if(!line.StartsWith("O "))
                {
                    lines.Add(line);
                    this.Log(LogLevel.Info, "{0}", line);
                    if(line.StartsWith("R "))
                    {
                        results.Add(line.Substring(2));
                    }
                    if(line.StartsWith("END "))
                    {
                        Result = line.Substring(4);
                    }
                    continue;
                }
                var parts = line.Split(' ');
                foreach(var b in Unhex(parts[1]))
                {
                    CharReceived?.Invoke(b);
                }
                var nrst = parts[2] == "1";
                if(nrstHeld && !nrst)
                {
                    Resets++;
                    machine.RequestReset();
                }
                nrstHeld = nrst;
                return;
            }
        }

        private static string Hex(List<byte> bytes)
        {
            if(bytes.Count == 0)
            {
                return "-";
            }
            var s = new StringBuilder();
            foreach(var b in bytes)
            {
                s.Append(b.ToString("X2"));
            }
            return s.ToString();
        }

        private static IEnumerable<byte> Unhex(string s)
        {
            if(s == "-")
            {
                yield break;
            }
            for(var i = 0; i + 1 < s.Length; i += 2)
            {
                yield return Convert.ToByte(s.Substring(i, 2), 16);
            }
        }

        private readonly IMachine machine;
        private readonly List<byte> rx;
        private readonly List<string> lines;
        private readonly List<string> results;
        private readonly LimitTimer tick;
        // a host process: not part of a saved emulation state
        [Transient]
        private Process process;
        private bool nrstHeld;
    }
}
