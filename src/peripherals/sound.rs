use super::super::mem::Memory;
use sdl2::AudioSubsystem;
use sdl2::audio::{AudioStatus, AudioDevice, AudioCallback, AudioSpecDesired};

use time;

// PulseAVoice registers
const NR10_REGISTER_ADDR: u16 = 0xFF10;
const NR11_REGISTER_ADDR: u16 = 0xFF11;
const NR12_REGISTER_ADDR: u16 = 0xFF12;
const NR13_REGISTER_ADDR: u16 = 0xFF13;
const NR14_REGISTER_ADDR: u16 = 0xFF14;

// PulseBReg registers
const NR21_REGISTER_ADDR: u16 = 0xFF16;
const NR22_REGISTER_ADDR: u16 = 0xFF17;
const NR23_REGISTER_ADDR: u16 = 0xFF18;
const NR24_REGISTER_ADDR: u16 = 0xFF19;

// Wave registers
const NR30_REGISTER_ADDR: u16 = 0xFF1A;
const NR31_REGISTER_ADDR: u16 = 0xFF1B;
const NR32_REGISTER_ADDR: u16 = 0xFF1C;
const NR33_REGISTER_ADDR: u16 = 0xFF1D;
const NR34_REGISTER_ADDR: u16 = 0xFF1E;

// Global sound registers
const NR50_REGISTER_ADDR: u16 = 0xFF24;
const NR51_REGISTER_ADDR: u16 = 0xFF25;
const NR52_REGISTER_ADDR: u16 = 0xFF26;

const CUSTOM_WAVE_START_ADDR: u16 = 0xFF30;
const CUSTOM_WAVE_END_ADDR: u16 = 0xFF3F;

// TODO make sure it is 44100 and not some other thing such as 48000.
const FREQ: i32 = 44100i32;
const SQUARE_DESIRED_SPEC: AudioSpecDesired = AudioSpecDesired {
    freq: Some(FREQ),
    channels: Some(2), // mono
    samples: None, // default sample size
};

#[derive(Copy, Clone, PartialEq, Debug)]
enum SweepFunc {
    Addition,
    Subtraction,
}

struct Sweep {
    shift_number: u8, // 0-7
    func: SweepFunc,
    sweep_time: u8,
}

impl Sweep {
    fn new(addr: u16, memory: &Memory) -> Self {
        let sweep_raw = memory.read_byte(addr);
        let shift_number = sweep_raw & 0b111;
        let func = if ((sweep_raw >> 3) & 0b1) == 0b0 {
            SweepFunc::Addition
        } else {
            SweepFunc::Subtraction
        };
        let sweep_time = (sweep_raw >> 4) & 0b111;

        Sweep {
            shift_number: shift_number,
            func: func,
            sweep_time: sweep_time,
        }
    }
    fn update(&mut self, addr: u16, memory: &Memory) {
        let sweep_raw = memory.read_byte(addr);
        self.shift_number = sweep_raw & 0b111;
        self.func = if ((sweep_raw >> 3) & 0b1) == 0b0 {
            SweepFunc::Addition
        } else {
            SweepFunc::Subtraction
        };
        self.sweep_time = (sweep_raw >> 4) & 0b111;
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum EnvelopeFunc {
    Attenuate,
    Amplify,
}

struct Envelope {
    step_length: u8, // 0-7
    func: EnvelopeFunc,
    default_value: u8, // 0x0-0xF
}

impl Envelope {
    fn new(addr: u16, memory: &Memory) -> Self {
        let envelope_raw = memory.read_byte(addr);
        let step_length = envelope_raw & 0b111;
        let func = if ((envelope_raw >> 3) & 0b1) == 0b0 {
            EnvelopeFunc::Attenuate
        } else {
            EnvelopeFunc::Amplify
        };
        let default_value = envelope_raw >> 4;

        Envelope {
            step_length: step_length,
            func: func,
            default_value: default_value,
        }
    }
    fn update(&mut self, addr: u16, memory: &Memory) {
        let envelope_raw = memory.read_byte(addr);
        self.step_length = envelope_raw & 0b111;
        self.func = if ((envelope_raw >> 3) & 0b1) == 0b0 {
            EnvelopeFunc::Attenuate
        } else {
            EnvelopeFunc::Amplify
        };
        self.default_value = envelope_raw >> 4;
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum SoundLoop {
    Loop,
    NoLoop,
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum SoundTrigger {
    On,
    Off,
}

struct PulseAVoice {
    sweep: Sweep,
    sound_length: u8, // 0-63
    waveform_duty_cycles: f32,
    envelope: Envelope,
    frequency: u16, // 11bits
    sound_loop: SoundLoop,
    sound_trigger: SoundTrigger,
    start_time: Option<time::Tm>,
    device: AudioDevice<SquareWave>,
}

impl PulseAVoice {
    fn new(audio_subsystem: &AudioSubsystem, memory: &Memory) -> Self {
        let nr11 = memory.read_byte(NR11_REGISTER_ADDR);
        let nr13 = memory.read_byte(NR13_REGISTER_ADDR);
        let nr14 = memory.read_byte(NR14_REGISTER_ADDR);

        let sound_length = nr11 & 0b0011_1111;
        let waveform_duty_cycles = match nr11 >> 6 {
            0b00 => 0.125f32,
            0b01 => 0.25f32,
            0b10 => 0.50f32,
            0b11 => 0.75f32,
            _ => unreachable!(),
        };
        let frequency = ((nr14 as u16 & 0b111) << 8) | nr13 as u16;
        let sound_loop = if ((nr14 >> 6) & 0b1) == 0b0 {
            SoundLoop::Loop
        } else {
            SoundLoop::NoLoop
        };
        let sound_trigger = if ((nr14 >> 7) & 0b1) == 0b0 {
            SoundTrigger::Off
        } else {
            SoundTrigger::On
        };

        PulseAVoice {
            sweep: Sweep::new(NR10_REGISTER_ADDR, memory),
            sound_length: sound_length,
            waveform_duty_cycles: waveform_duty_cycles,
            envelope: Envelope::new(NR12_REGISTER_ADDR, memory),
            frequency: frequency,
            sound_loop: sound_loop,
            sound_trigger: sound_trigger,
            start_time: None,
            device: audio_subsystem
                .open_playback(None, &SQUARE_DESIRED_SPEC, |_| {
                    SquareWave {
                        phase_inc: 0f32,
                        phase: 0f32,
                        volume: 0f32,
                        duty: 0f32,
                    }
                })
                .unwrap(),
        }
    }

    fn update(&mut self, memory: &mut Memory) {
        let nr11 = memory.read_byte(NR11_REGISTER_ADDR);
        let nr13 = memory.read_byte(NR13_REGISTER_ADDR);
        let nr14 = memory.read_byte(NR14_REGISTER_ADDR);

        let sound_length = nr11 & 0b0011_1111;
        let waveform_duty_cycles = match nr11 >> 6 {
            0b00 => 0.125f32,
            0b01 => 0.25f32,
            0b10 => 0.50f32,
            0b11 => 0.75f32,
            _ => unreachable!(),
        };
        let frequency = ((nr14 as u16 & 0b111) << 8) | nr13 as u16;
        let sound_loop = if ((nr14 >> 6) & 0b1) == 0b0 {
            SoundLoop::Loop
        } else {
            SoundLoop::NoLoop
        };

        let sound_trigger = if ((nr14 >> 7) & 0b1) == 0b1 {
            SoundTrigger::On
        } else {
            SoundTrigger::Off
        };

        self.sweep.update(NR10_REGISTER_ADDR, memory);
        self.sound_length = sound_length;
        self.waveform_duty_cycles = waveform_duty_cycles;
        self.envelope.update(NR12_REGISTER_ADDR, memory);
        self.frequency = frequency;
        self.sound_loop = sound_loop;
        self.sound_trigger = sound_trigger;
    }

    fn update_device(&mut self) {
        let frequency_hz = 131072f32 / (2048f32 - self.frequency as f32);
        let mut lock = self.device.lock();
        (*lock).phase_inc = frequency_hz / FREQ as f32;
        (*lock).volume = self.envelope.default_value as f32;
        (*lock).duty = self.waveform_duty_cycles;
    }

    fn elapsed_time(&self) -> time::Duration {
        if self.start_time.is_none() {
            time::Duration::milliseconds(0)
        } else {
            time::now() - self.start_time.unwrap()
        }
    }

    fn run(&mut self, memory: &mut Memory) {
        self.update(memory);
        // TODO: should we also check if at least one output channel is on?
        //(GlobalReg::should_output(VoiceType::PulseA, ChannelNum::ChannelA) ||
        // GlobalReg::should_output(VoiceType::PulseA, ChannelNum::ChannelB))
        if self.sound_trigger == SoundTrigger::On {
            GlobalReg::set_voice_flag(VoiceType::PulseA, memory);
            if self.sound_loop == SoundLoop::NoLoop {
                // sound length has elapsed?
                // TODO: instead of millis, use nanos?
                let sound_length = ((64f32 - self.sound_length as f32) * (1f32 / 256f32) *
                                        1000f32) as i64; // millis
                if self.elapsed_time() >= time::Duration::milliseconds(sound_length) {
                    self.stop(memory);
                    return;
                }
            }

            // handle sweep
            if self.sweep.sweep_time > 0 {
                // only apply sweep if passed sweep time
                // TODO: as i64 may cut some values, probably best to use nanoseconds and multiply
                // sweep_time by 10^9.
                if self.elapsed_time() >
                    time::Duration::milliseconds(
                        ((self.sweep.sweep_time as f32 / 128f32) * 1000f32) as i64,
                    )
                {
                    let mult = match self.sweep.func {
                        SweepFunc::Addition => 1f32,
                        SweepFunc::Subtraction => -1f32,
                    };
                    let new_freq = (self.frequency as f32 +
                                        (mult *
                                             (self.frequency as f32 /
                                                  2f32.powi(self.sweep.shift_number as i32)))) as
                        u16;
                    if new_freq > 0b0000_0111_1111_1111 {
                        self.stop(memory);
                    } else {
                        let nr13 = memory.read_byte(NR13_REGISTER_ADDR);
                        let nr14 = memory.read_byte(NR14_REGISTER_ADDR);
                        memory.write_byte(NR13_REGISTER_ADDR, new_freq as u8);
                        memory.write_byte(
                            NR14_REGISTER_ADDR,
                            nr14 | ((new_freq >> 8) & 0b111) as u8,
                        );
                        self.update_device();
                    }
                }
            }

            // handle envelope
            if self.envelope.step_length > 0 {
                // TODO: use nanos instead?
                let len_millis = (self.envelope.step_length as f32 * (1000f32 / 64f32)) as i64;
                if self.elapsed_time() >= time::Duration::milliseconds(len_millis) {
                    let mult = match self.envelope.func {
                        EnvelopeFunc::Amplify => 1i16,
                        EnvelopeFunc::Attenuate => -1i16,
                    };
                    let new_value = (self.envelope.default_value as i16 + mult) as u8;
                    if new_value < 0xF {
                        let nr12 = memory.read_byte(NR12_REGISTER_ADDR);
                        memory.write_byte(
                            NR12_REGISTER_ADDR,
                            (nr12 & 0b0000_1111) | (new_value << 4),
                        );
                        self.update_device();
                    }
                }
            }

            if self.start_time.is_none() {
                // first loop with sound on.
                // things here should be run only once when the sound is on.
                self.update_device();
                self.device.resume();
                self.start_time = Some(time::now());
                // TODO: maybe set the voice flag everytime just to be sure it will be correct for
                // the duration of the sound?
            }
        } else {
            self.stop(memory);
        }
    }

    fn stop(&mut self, memory: &mut Memory) {
        // this if is for extra safety
        if self.device.status() == AudioStatus::Playing {
            self.device.pause();

            let mut lock = self.device.lock();
            (*lock).phase = 0.0;

            self.start_time = None;
            GlobalReg::reset_voice_flag(VoiceType::PulseA, memory);
            // reset initialize (trigger) flag
            let nr14 = memory.read_byte(NR14_REGISTER_ADDR);
            memory.write_byte(NR14_REGISTER_ADDR, nr14 & 0b0111_1111);
        }
    }
}

enum VoiceType {
    PulseA,
    PulseB,
    Wave,
    WhiteNoise,
}

impl VoiceType {
    fn global_mask(&self) -> u8 {
        // bit position in the global registers
        let global_position: u8 = match *self {
            VoiceType::PulseA => 0,
            VoiceType::PulseB => 1,
            VoiceType::Wave => 2,
            VoiceType::WhiteNoise => 3,
        };
        1 << global_position
    }
}

enum ChannelNum {
    ChannelA,
    ChannelB,
}

struct GlobalReg;

impl GlobalReg {
    fn should_output(voice_type: VoiceType, channel_num: ChannelNum, memory: &Memory) -> bool {
        let channel_nibble = match channel_num {
            ChannelNum::ChannelA => 0,
            ChannelNum::ChannelB => 1,
        };
        let nr51 = memory.read_byte(NR51_REGISTER_ADDR);
        (nr51 >> (4 * channel_nibble) & voice_type.global_mask()) == voice_type.global_mask()
    }
    fn reset_voice_flag(voice_type: VoiceType, memory: &mut Memory) {
        let nr52 = memory.read_byte(NR52_REGISTER_ADDR);
        memory.write_byte(NR52_REGISTER_ADDR, nr52 & !(voice_type.global_mask()));
    }
    fn set_voice_flag(voice_type: VoiceType, memory: &mut Memory) {
        let nr52 = memory.read_byte(NR52_REGISTER_ADDR);
        memory.write_byte(NR52_REGISTER_ADDR, nr52 | voice_type.global_mask());
    }
}

pub struct SoundController {
    sound_is_on: bool,
    channel_1_volume: u8,
    channel_2_volume: u8,
    pulse_a: PulseAVoice,
}

impl SoundController {
    pub fn new(audio_subsystem: &AudioSubsystem, memory: &Memory) -> Self {
        SoundController {
            sound_is_on: false,
            channel_1_volume: 0,
            channel_2_volume: 0,
            pulse_a: PulseAVoice::new(audio_subsystem, memory),
        }
    }
    pub fn reset(&mut self, memory: &mut Memory) {
        if self.pulse_a.sound_trigger == SoundTrigger::On {
            self.pulse_a.stop(memory);
        }
    }

    pub fn run(&mut self, memory: &mut Memory) {
        let sound_onoff = memory.read_byte(NR52_REGISTER_ADDR);

        self.sound_is_on = (sound_onoff >> 7) == 0b1;
        if self.sound_is_on {
            let channel_ctrl = memory.read_byte(NR50_REGISTER_ADDR);
            self.channel_1_volume = channel_ctrl & 0b111;
            self.channel_2_volume = (channel_ctrl >> 4) & 0b111;

            self.pulse_a.run(memory);
        } else {
            self.reset(memory);
        }
    }
}

struct SquareWave {
    phase_inc: f32,
    phase: f32,
    volume: f32,
    duty: f32,
}

impl AudioCallback for SquareWave {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        // Generate a square wave
        for x in out.iter_mut() {
            *x = if self.phase <= self.duty {
                self.volume
            } else {
                -self.volume
            };
            self.phase = (self.phase + self.phase_inc) % 1.0;
        }
    }
}
