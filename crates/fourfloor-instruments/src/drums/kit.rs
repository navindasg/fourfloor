//! The drum kit: every drum voice in one device, played from General MIDI
//! drum pitches.

use fourfloor_core::{
    velocity_to_unit, Device, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo,
    TransportContext,
};

use super::{
    ClapVoice, Cowbell808Voice, DrumVoice, HatVoice, Kick808Voice, Kick909Voice, RimVoice,
    Snare909Voice, TomVoice,
};
use crate::runtime::{clamped, run, Synth};

/// One voice of a [`DrumKit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KitSlot {
    /// 909 kick. GM pitch 36.
    Kick909,
    /// 808 kick. GM pitch 35.
    Kick808,
    /// 909 snare. GM pitches 38 and 40.
    Snare,
    /// Hand clap. GM pitch 39.
    Clap,
    /// Hi-hat. GM pitches 42 and 44 (closed) and 46 (open).
    Hat,
    /// Rim shot. GM pitch 37.
    Rim,
    /// Low tom. GM pitches 41 and 43.
    TomLow,
    /// Mid tom. GM pitches 45 and 47.
    TomMid,
    /// High tom. GM pitches 48 and 50.
    TomHigh,
    /// 808 cowbell. GM pitch 56.
    Cowbell,
}

impl KitSlot {
    /// Every slot, in parameter order.
    pub const ALL: [Self; 10] = [
        Self::Kick909,
        Self::Kick808,
        Self::Snare,
        Self::Clap,
        Self::Hat,
        Self::Rim,
        Self::TomLow,
        Self::TomMid,
        Self::TomHigh,
        Self::Cowbell,
    ];

    /// Display name, used as the prefix of the slot's parameter names.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Kick909 => "Kick 909",
            Self::Kick808 => "Kick 808",
            Self::Snare => "Snare",
            Self::Clap => "Clap",
            Self::Hat => "Hat",
            Self::Rim => "Rim",
            Self::TomLow => "Tom Low",
            Self::TomMid => "Tom Mid",
            Self::TomHigh => "Tom High",
            Self::Cowbell => "Cowbell",
        }
    }

    const fn index(self) -> u32 {
        self as u32
    }

    /// The slot a MIDI pitch plays, following the General MIDI drum map.
    /// `None` for pitches the kit has no voice for.
    pub const fn for_pitch(pitch: u8) -> Option<Self> {
        Some(match pitch {
            35 => Self::Kick808,
            36 => Self::Kick909,
            37 => Self::Rim,
            38 | 40 => Self::Snare,
            39 => Self::Clap,
            42 | 44 | 46 => Self::Hat,
            41 | 43 => Self::TomLow,
            45 | 47 => Self::TomMid,
            48 | 50 => Self::TomHigh,
            56 => Self::Cowbell,
            _ => return None,
        })
    }

    fn table(self) -> &'static [ParamInfo] {
        match self {
            Self::Kick909 => Kick909Voice::param_table(),
            Self::Kick808 => Kick808Voice::param_table(),
            Self::Snare => Snare909Voice::param_table(),
            Self::Clap => ClapVoice::param_table(),
            Self::Hat => HatVoice::param_table(),
            Self::Rim => RimVoice::param_table(),
            Self::TomLow | Self::TomMid | Self::TomHigh => TomVoice::param_table(),
            Self::Cowbell => Cowbell808Voice::param_table(),
        }
    }

    /// Tuning in Hz the kit gives a tom slot.
    const fn tom_tune(self) -> Option<f32> {
        match self {
            Self::TomLow => Some(95.0),
            Self::TomMid => Some(130.0),
            Self::TomHigh => Some(175.0),
            _ => None,
        }
    }
}

/// All the drum voices in one device, played from General MIDI drum
/// pitches (see [`KitSlot`]). Kind `drum.kit`.
///
/// Parameters: [`DrumKit::LEVEL`], then every voice's own parameters at
/// [`DrumKit::param`]`(slot, voice_param)`, named with the slot's name in
/// front ("Kick 909 Tune"). The closed hat chokes the open hat. Output is
/// the same on both channels.
#[derive(Clone, Debug)]
pub struct DrumKit {
    kick909: Kick909Voice,
    kick808: Kick808Voice,
    snare: Snare909Voice,
    clap: ClapVoice,
    hat: HatVoice,
    rim: RimVoice,
    toms: [TomVoice; 3],
    cowbell: Cowbell808Voice,
    level: f32,
    params: Vec<ParamInfo>,
}

impl DrumKit {
    /// Master level of the kit, 0 to 1.
    pub const LEVEL: ParamId = ParamId(0);

    /// Ids each slot reserves for its voice's parameters.
    const SLOT_STRIDE: u32 = 16;

    /// The kit parameter that controls `voice_param` of the voice in
    /// `slot`. `voice_param` is one of the voice's own ids, for example
    /// [`Kick909Voice::TUNE`].
    pub const fn param(slot: KitSlot, voice_param: ParamId) -> ParamId {
        ParamId((slot.index() + 1) * Self::SLOT_STRIDE + voice_param.0)
    }

    /// A kit with default parameters.
    pub fn new() -> Self {
        let mut params = vec![ParamInfo::new(
            Self::LEVEL,
            "Level",
            0.0,
            1.0,
            0.8,
            ParamUnit::Generic,
        )];
        for slot in KitSlot::ALL {
            for info in slot.table() {
                let mut info = info
                    .clone()
                    .with_name(format!("{} {}", slot.name(), info.name));
                info.id = Self::param(slot, info.id);
                if let (Some(tune), true) = (slot.tom_tune(), info.id.0 % 16 == TomVoice::TUNE.0) {
                    info.default = tune;
                }
                params.push(info);
            }
        }
        let mut kit = Self {
            kick909: Kick909Voice::default(),
            kick808: Kick808Voice::default(),
            snare: Snare909Voice::default(),
            clap: ClapVoice::default(),
            hat: HatVoice::default(),
            rim: RimVoice::default(),
            toms: [
                TomVoice::default(),
                TomVoice::default(),
                TomVoice::default(),
            ],
            cowbell: Cowbell808Voice::default(),
            level: 0.8,
            params,
        };
        for (tom, slot) in
            kit.toms
                .iter_mut()
                .zip([KitSlot::TomLow, KitSlot::TomMid, KitSlot::TomHigh])
        {
            if let Some(tune) = slot.tom_tune() {
                tom.set_param(TomVoice::TUNE, tune);
            }
        }
        kit
    }

    fn voice_mut(&mut self, slot: KitSlot) -> &mut dyn DrumVoice {
        match slot {
            KitSlot::Kick909 => &mut self.kick909,
            KitSlot::Kick808 => &mut self.kick808,
            KitSlot::Snare => &mut self.snare,
            KitSlot::Clap => &mut self.clap,
            KitSlot::Hat => &mut self.hat,
            KitSlot::Rim => &mut self.rim,
            KitSlot::TomLow => &mut self.toms[0],
            KitSlot::TomMid => &mut self.toms[1],
            KitSlot::TomHigh => &mut self.toms[2],
            KitSlot::Cowbell => &mut self.cowbell,
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        match pitch {
            35 => self.kick808.trigger(pitch, velocity),
            36 => self.kick909.trigger(pitch, velocity),
            37 => self.rim.trigger(pitch, velocity),
            38 | 40 => self.snare.trigger(pitch, velocity),
            39 => self.clap.trigger(pitch, velocity),
            42 | 44 => self.hat.trigger_closed(velocity),
            46 => self.hat.trigger_open(velocity),
            // Each tom slot covers two GM pitches a little apart.
            41 => self.toms[0].trigger_scaled(0.84, velocity),
            43 => self.toms[0].trigger_scaled(1.0, velocity),
            45 => self.toms[1].trigger_scaled(0.84, velocity),
            47 => self.toms[1].trigger_scaled(1.0, velocity),
            48 => self.toms[2].trigger_scaled(1.0, velocity),
            50 => self.toms[2].trigger_scaled(1.19, velocity),
            56 => self.cowbell.trigger(pitch, velocity),
            _ => {}
        }
    }
}

impl Default for DrumKit {
    fn default() -> Self {
        Self::new()
    }
}

impl Synth for DrumKit {
    fn handle_event(&mut self, kind: EventKind) {
        if let EventKind::NoteOn {
            pitch, velocity, ..
        } = kind
        {
            if velocity > 0 {
                self.note_on(pitch, velocity_to_unit(velocity));
            }
        }
    }

    fn tick(&mut self) -> (f32, f32) {
        let mut sum = self.kick909.tick()
            + self.kick808.tick()
            + self.snare.tick()
            + self.clap.tick()
            + self.hat.tick()
            + self.rim.tick()
            + self.cowbell.tick();
        for tom in &mut self.toms {
            sum += tom.tick();
        }
        let out = sum * self.level;
        (out, out)
    }
}

impl Device for DrumKit {
    fn prepare(&mut self, config: &PrepareConfig) {
        let rate = config.sample_rate.hz() as f32;
        for slot in KitSlot::ALL {
            let voice = self.voice_mut(slot);
            voice.prepare(rate);
            voice.reset();
        }
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        run(self, io);
    }

    fn params(&self) -> &[ParamInfo] {
        &self.params
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        let Some(value) = clamped(&self.params, param, value) else {
            return;
        };
        if param == Self::LEVEL {
            self.level = value;
            return;
        }
        let slot_index = (param.0 / Self::SLOT_STRIDE) as usize;
        if let Some(slot) = slot_index
            .checked_sub(1)
            .and_then(|index| KitSlot::ALL.get(index).copied())
        {
            self.voice_mut(slot)
                .set_param(ParamId(param.0 % Self::SLOT_STRIDE), value);
        }
    }

    fn reset(&mut self) {
        for slot in KitSlot::ALL {
            self.voice_mut(slot).reset();
        }
    }
}
