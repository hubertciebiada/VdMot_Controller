//
// ADC1 of the STM32F4 with the twelve valve motors of the VdMot controller behind its inputs, for
// the Renode tests of the Rust STM32 images (docs/rust/GLUE-DESIGN-STM.md §5.7). Compiled by
// Renode at run time (include @VdmValves.cs). A reduced port of the valve sim of the C++ glue
// suites (software_stm32/test/native/glue/sim, same defaults): every quarter millisecond of
// virtual time the motor that the outputs select (valve PSU on: PB9 low; its L293 enable
// ENA0..5 high: PA5, PA6, PA7, PB0, PA15, PB3; the MUX relay PB1 picks the even or the odd valve;
// DIR PA8 low = opening) turns: it draws its running current, or its stall current at an end
// stop, and gives revolution pulses on REVIN (PA4, the GPIO output Revin), at most one per
// quarter millisecond, so every pulse is its own EXTI4 edge. ADC1 converts at once: channel 0
// (PA0) is the current input (the inverse of TimerHandler0's conversion around 2048), channel 1
// (PA1) the reference 2048. No inrush, no coast, no short, no obstacle.
//

using System;
using System.Collections.Generic;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Analog
{
    public class VdmValves : IDoubleWordPeripheral, IWordPeripheral, IKnownSize
    {
        public VdmValves(IMachine machine)
        {
            this.machine = machine;
            Revin = new GPIO();
            registers = new Dictionary<long, uint>();
            valves = new Valve[ValveCount];
            for(var i = 0; i < ValveCount; i++)
            {
                valves[i] = new Valve();
            }
            tick = new LimitTimer(machine.ClockSource, 1000 * TicksPerMs, this, "valve_tick", limit: 1, direction: Direction.Ascending, enabled: true, eventEnabled: true, autoUpdate: true);
            tick.LimitReached += OnTick;
            lastEnabled = -1;
        }

        public long Size => 0x400;

        // revolution pulses (connected to PA4 in the platform)
        public GPIO Revin { get; }

        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
                case SrOffset:
                    // STRT | EOC: every conversion is done at once
                    return 0x12;
                case DrOffset:
                    var channel = Get(Sqr3Offset) & 0x1F;
                    return channel == 0 ? CurrentCounts() : (channel == 1 ? AdcMid : 0u);
                default:
                    return Get(offset);
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            registers[offset] = value;
        }

        // embassy reads ADC_DR by halfword (ldrh)
        public ushort ReadWord(long offset)
        {
            var word = ReadDoubleWord(offset & ~3L);
            return (ushort)(word >> (int)(8 * (offset & 2)));
        }

        public void WriteWord(long offset, ushort value)
        {
            this.Log(LogLevel.Warning, "halfword write at 0x{0:X}: the ADC takes words only", offset);
        }

        public void Reset()
        {
            // the valves stay where they are: only the ADC and the motor drive start again
            registers.Clear();
            current = 0;
            lastEnabled = -1;
        }

        // ---- the tests (monitor: sysbus.valves <Method> <args>)

        public void SetConnected(int valve, bool connected)
        {
            valves[valve].Connected = connected;
        }

        public void SetPosition(int valve, int position)
        {
            valves[valve].Position = position;
        }

        public void SetStroke(int valve, int stroke)
        {
            valves[valve].Stroke = stroke;
        }

        // motor speed of every valve (the C++ system suites: 0.2 in the boot suite, else 2.0)
        public void SetSpeed(double pulsesPerMs)
        {
            foreach(var v in valves)
            {
                v.PulsesPerMs = pulsesPerMs;
            }
        }

        public int GetPosition(int valve)
        {
            return valves[valve].Position;
        }

        public long GetEnables(int valve)
        {
            return valves[valve].Enables;
        }

        public long GetPulses(int valve)
        {
            return valves[valve].Pulses;
        }

        // "enables pulses position" of every valve, for the test logs
        public string Summary()
        {
            var parts = new List<string>();
            foreach(var v in valves)
            {
                parts.Add(string.Format("{0}/{1}/{2}", v.Enables, v.Pulses, v.Position));
            }
            return string.Join(" ", parts);
        }

        public int Current => current;

        // ticks with more than one L293 enable on
        public long Conflicts { get; private set; }

        // the MUX level that selects the even valve: high on C1 boards, low on C2
        public bool MuxOnHigh { get; set; }

        private void OnTick()
        {
            var bus = machine.SystemBus;
            var a = bus.ReadDoubleWord(GpioaOdr);
            var b = bus.ReadDoubleWord(GpiobOdr);
            var powered = (b & (1u << 9)) == 0;
            var dir = (a & (1u << 8)) == 0 ? 1 : -1;
            var e = powered ? EnabledValve(a, b) : -1;
            if(e != lastEnabled)
            {
                if(e >= 0)
                {
                    valves[e].Enables++;
                    accumulator = 0;
                }
                lastEnabled = e;
            }
            current = 0;
            if(e < 0 || !valves[e].Connected)
            {
                return;
            }
            var v = valves[e];
            var stalled = Stalled(v, dir);
            current = dir * (stalled ? v.StallCurrent : v.RunCurrent);
            if(stalled)
            {
                accumulator = 0;
                return;
            }
            accumulator += v.PulsesPerMs / TicksPerMs;
            if(accumulator >= 1.0)
            {
                accumulator -= 1.0;
                v.Position += dir;
                v.Pulses++;
                Revin.Set(true);
                Revin.Set(false);
            }
        }

        // the valve whose enable and MUX position are active, -1 if none
        private int EnabledValve(uint a, uint b)
        {
            var enables = new bool[]
            {
                (a & (1u << 5)) != 0, (a & (1u << 6)) != 0, (a & (1u << 7)) != 0,
                (b & (1u << 0)) != 0, (a & (1u << 15)) != 0, (b & (1u << 3)) != 0
            };
            var muxHigh = (b & (1u << 1)) != 0;
            var found = -1;
            for(var k = 0; k < enables.Length; k++)
            {
                if(!enables[k])
                {
                    continue;
                }
                if(found >= 0)
                {
                    Conflicts++;
                    break;
                }
                var even = muxHigh == MuxOnHigh;
                found = 2 * k + (even ? 0 : 1);
            }
            return found;
        }

        private static bool Stalled(Valve v, int dir)
        {
            return dir > 0 ? v.Position >= v.Stroke : v.Position <= 0;
        }

        // the inverse of TimerHandler0: current (0.1 mA) = (adc - 2048) * 138 / 100
        private uint CurrentCounts()
        {
            var counts = (long)AdcMid + (long)Math.Round(current * 100.0 / 138.0);
            return (uint)Math.Max(0, Math.Min(4095, counts));
        }

        private uint Get(long offset)
        {
            return registers.TryGetValue(offset, out var v) ? v : 0u;
        }

        private class Valve
        {
            public bool Connected = true;
            public int Position = 1800;
            public int Stroke = 3600;
            public double PulsesPerMs = 0.2;
            public int RunCurrent = 250;
            public int StallCurrent = 700;
            public long Enables;
            public long Pulses;
        }

        private readonly IMachine machine;
        private readonly Dictionary<long, uint> registers;
        private readonly Valve[] valves;
        private readonly LimitTimer tick;
        private int current;
        private int lastEnabled;
        private double accumulator;

        private const int ValveCount = 12;
        private const int TicksPerMs = 4;
        private const uint AdcMid = 2048;
        private const long SrOffset = 0x00;
        private const long Sqr3Offset = 0x34;
        private const long DrOffset = 0x4C;
        private const ulong GpioaOdr = 0x40020014;
        private const ulong GpiobOdr = 0x40020414;
    }
}
