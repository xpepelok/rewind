use crate::wire::{self, Value, Writer};

pub const VERSION: (u32, u32, u32) = (7, 9, 0);
pub const KIND: u32 = 0;
pub const CAMERA: (&str, &str) = ("Ansel", "NvCamera");
pub const CONTROL: (&str, &str) = ("Ansel", "NvCameraControl");
pub const NO_FILTER: &str = "None";

const REQUEST: u32 = 1;
const RESPONSE: u32 = 2;

pub mod status {
    pub const OK: u32 = 1;
    pub const STARTED: u32 = 3;
    pub const ALREADY_ENABLED: u32 = 10;
    pub const ALREADY_DISABLED: u32 = 11;
    pub const ALREADY_SET: u32 = 13;
    pub const OK_PHOTO: u32 = 16;
    pub const OK_FILTERS_ONLY: u32 = 17;
    pub const NEEDS_DEPTH: u32 = 18;
    pub const SHOT_WITH_UI_SAVED: u32 = 21;
    pub const PROCESSING_DONE: u32 = 26;
    pub const SHOT_SAVED: u32 = 28;

    pub fn succeeded(status: u32) -> bool {
        matches!(
            status,
            OK | STARTED
                | ALREADY_ENABLED
                | ALREADY_DISABLED
                | ALREADY_SET
                | OK_PHOTO
                | OK_FILTERS_ONLY
                | SHOT_WITH_UI_SAVED
                | PROCESSING_DONE
                | SHOT_SAVED
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shot {
    Regular,
    Stereo,
    Highres,
    Panorama,
    PanoramaStereo,
}

impl Shot {
    pub const ALL: [Self; 5] = [
        Self::Regular,
        Self::Stereo,
        Self::Highres,
        Self::Panorama,
        Self::PanoramaStereo,
    ];

    const fn code(self) -> u64 {
        match self {
            Self::Regular => 0,
            Self::Stereo => 1,
            Self::Highres => 2,
            Self::Panorama => 3,
            Self::PanoramaStereo => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capture {
    pub shot: Shot,
    pub multiplier: Option<i32>,
    pub panorama_width: Option<i32>,
    pub exr: bool,
    pub enhance: bool,
    pub jxr: bool,
}

impl Capture {
    pub const fn regular() -> Self {
        Self {
            shot: Shot::Regular,
            multiplier: None,
            panorama_width: None,
            exr: false,
            enhance: false,
            jxr: false,
        }
    }

    fn body(&self, estimate: bool) -> Writer {
        let mut body = Writer::new().uint(1, self.shot.code());
        if let Some(multiplier) = self.multiplier {
            body = body.uint(2, signed(multiplier));
        }
        if let Some(width) = self.panorama_width {
            body = body.uint(3, signed(width));
        }
        body = body.flag(4, false).flag(5, self.exr).flag(6, self.enhance);
        if estimate {
            body
        } else {
            body.flag(7, self.jxr)
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    Floats(Vec<f32>),
    Bools(Vec<bool>),
    Uints(Vec<u32>),
    Ints(Vec<i32>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControlChange {
    Pressed,
    Slider(f32),
    Toggle(bool),
    Selected(i32),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Version,
    Available,
    Process,
    Features,
    EnabledFeatures,
    UiReady,
    Enable {
        on: bool,
        keep_filters: bool,
        pause: bool,
    },
    IsEnabled,
    Filters,
    Stack,
    FilterInfo {
        slot: i32,
    },
    SetFilter {
        filter: String,
        slot: i32,
    },
    InsertFilter {
        filter: String,
        slot: i32,
    },
    RemoveFilter {
        slot: i32,
    },
    MoveFilters {
        order: Vec<u32>,
    },
    SetAttribute {
        filter: String,
        slot: i32,
        control: i32,
        values: Values,
    },
    ResetFilter {
        slot: i32,
    },
    ResetAllValues,
    ResetStack,
    FovRange,
    Fov,
    SetFov(f32),
    RollRange,
    SetRoll(f32),
    ShotPermissions,
    HighresList,
    PanoramaRange,
    Capture(Capture),
    Estimate(Capture),
    AbortCapture,
    ScreenResolution,
    ResolutionChanged,
    GridOfThirds(bool),
    HighQuality(bool),
    Language {
        language: i32,
        sublanguage: i32,
    },
    Settings,
    Input {
        message: i32,
        wparam: i32,
        lparam: i32,
        delta: bool,
    },
    ControlChanged {
        control: i32,
        change: ControlChange,
    },
}

impl Request {
    fn field(&self) -> u32 {
        match self {
            Self::Enable { .. } => 1,
            Self::Capture(_) => 2,
            Self::Filters => 3,
            Self::HighresList => 4,
            Self::AbortCapture => 5,
            Self::PanoramaRange => 6,
            Self::FovRange => 7,
            Self::SetFov(_) => 8,
            Self::RollRange => 9,
            Self::SetRoll(_) => 10,
            Self::IsEnabled => 11,
            Self::Fov => 12,
            Self::SetFilter { .. } => 13,
            Self::SetAttribute { .. } => 14,
            Self::Available => 17,
            Self::ResolutionChanged => 18,
            Self::ShotPermissions => 19,
            Self::Input { .. } => 20,
            Self::Language { .. } => 21,
            Self::FilterInfo { .. } => 22,
            Self::ResetFilter { .. } => 23,
            Self::ResetAllValues => 24,
            Self::Version => 26,
            Self::UiReady => 28,
            Self::ControlChanged { .. } => 29,
            Self::Process => 33,
            Self::ScreenResolution => 34,
            Self::GridOfThirds(_) => 36,
            Self::ResetStack => 37,
            Self::Features => 38,
            Self::InsertFilter { .. } => 44,
            Self::RemoveFilter { .. } => 45,
            Self::MoveFilters { .. } => 46,
            Self::Stack => 47,
            Self::Estimate(_) => 48,
            Self::Settings => 50,
            Self::EnabledFeatures => 56,
            Self::HighQuality(_) => 59,
        }
    }

    fn body(&self) -> Writer {
        let body = Writer::new();
        match self {
            Self::Version
            | Self::Available
            | Self::Process
            | Self::EnabledFeatures
            | Self::IsEnabled
            | Self::Filters
            | Self::Stack
            | Self::ResetAllValues
            | Self::ResetStack
            | Self::FovRange
            | Self::Fov
            | Self::RollRange
            | Self::ShotPermissions
            | Self::HighresList
            | Self::PanoramaRange
            | Self::AbortCapture
            | Self::ScreenResolution
            | Self::ResolutionChanged
            | Self::Settings => body,
            Self::UiReady => body.uint(1, u64::from(status::OK)),
            Self::Features => body.message(1, version()),
            Self::Enable {
                on,
                keep_filters,
                pause,
            } => body
                .uint(1, u64::from(VERSION.0))
                .uint(2, u64::from(VERSION.1))
                .uint(3, u64::from(VERSION.2))
                .flag(4, *on)
                .flag(5, *keep_filters)
                .flag(6, *pause),
            Self::FilterInfo { slot }
            | Self::RemoveFilter { slot }
            | Self::ResetFilter { slot } => body.uint(1, signed(*slot)),
            Self::SetFilter { filter, slot } | Self::InsertFilter { filter, slot } => {
                body.text(1, filter).uint(2, signed(*slot))
            }
            Self::MoveFilters { order } => order
                .iter()
                .fold(body, |body, index| body.uint(1, u64::from(*index))),
            Self::SetAttribute {
                filter,
                slot,
                control,
                values,
            } => {
                let body = body
                    .text(1, filter)
                    .uint(2, signed(*slot))
                    .uint(3, signed(*control));
                match values {
                    Values::Floats(list) => {
                        list.iter().fold(body, |body, value| body.float(4, *value))
                    }
                    Values::Bools(list) => {
                        list.iter().fold(body, |body, value| body.flag(5, *value))
                    }
                    Values::Uints(list) => list
                        .iter()
                        .fold(body, |body, value| body.uint(6, u64::from(*value))),
                    Values::Ints(list) => list
                        .iter()
                        .fold(body, |body, value| body.uint(7, signed(*value))),
                }
            }
            Self::SetFov(value) | Self::SetRoll(value) => body.float(1, *value),
            Self::Capture(capture) => capture.body(false),
            Self::Estimate(capture) => capture.body(true),
            Self::GridOfThirds(on) | Self::HighQuality(on) => body.flag(1, *on),
            Self::Language {
                language,
                sublanguage,
            } => body
                .uint(1, signed(*language))
                .uint(2, signed(*sublanguage)),
            Self::Input {
                message,
                wparam,
                lparam,
                delta,
            } => body
                .uint(1, signed(*message))
                .uint(2, signed(*wparam))
                .uint(3, signed(*lparam))
                .flag(4, *delta),
            Self::ControlChanged { control, change } => {
                let id = Writer::new().uint(1, signed(*control));
                match change {
                    ControlChange::Pressed => body.message(3, id),
                    ControlChange::Slider(value) => body.message(4, id.float(5, *value)),
                    ControlChange::Text(text) => body.message(5, id.text(4, text)),
                    ControlChange::Toggle(on) => body.message(6, id.flag(3, *on)),
                    ControlChange::Selected(key) => body.message(8, id.uint(4, signed(*key))),
                }
            }
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let request = Writer::new().message(self.field(), self.body());
        Writer::new().message(REQUEST, request).finish()
    }
}

fn version() -> Writer {
    Writer::new()
        .uint(1, u64::from(VERSION.0))
        .uint(2, u64::from(VERSION.1))
        .uint(3, u64::from(VERSION.2))
}

fn signed(value: i32) -> u64 {
    i64::from(value) as u64
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    Slider,
    Boolean,
    Button,
    List,
    Edit,
    Label,
    Pulldown,
    ColorPicker,
    RadioButton,
}

impl ControlKind {
    fn from_code(code: u32) -> Self {
        match code {
            2 => Self::Boolean,
            3 => Self::Button,
            4 => Self::List,
            5 => Self::Edit,
            6 => Self::Label,
            7 => Self::Pulldown,
            8 => Self::ColorPicker,
            9 => Self::RadioButton,
            _ => Self::Slider,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    Float,
    Bool,
    Int,
    Uint,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub current: f32,
    pub default: f32,
    pub minimum: f32,
    pub maximum: f32,
    pub step: f32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub id: i32,
    pub name: String,
    pub kind: ControlKind,
    pub scalar: Scalar,
    pub minimum_shown: f32,
    pub maximum_shown: f32,
    pub precision: i32,
    pub unit: String,
    pub labels: Vec<String>,
    pub tooltip: String,
    pub components: Vec<Component>,
}

impl Control {
    pub fn values(&self) -> Values {
        let current = self.components.iter().map(|component| component.current);
        match self.scalar {
            Scalar::Float => Values::Floats(current.collect()),
            Scalar::Bool => Values::Bools(current.map(|value| value != 0.0).collect()),
            Scalar::Int => Values::Ints(current.map(|value| value.round() as i32).collect()),
            Scalar::Uint => {
                Values::Uints(current.map(|value| value.round().max(0.0) as u32).collect())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Properties {
    pub id: String,
    pub name: String,
    pub controls: Vec<Control>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Highres {
    pub multiplier: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Version(u32, u32, u32),
    Available(bool),
    Process {
        id: u32,
        path: String,
    },
    Features {
        sdk: bool,
        filters: bool,
        restyle: bool,
    },
    EnabledFeatures {
        filters: bool,
    },
    UiReady,
    Enabled(u32),
    IsEnabled(bool),
    Filters(Vec<Filter>),
    Stack(Vec<String>),
    FilterSet {
        status: u32,
        slot: i32,
        properties: Option<Properties>,
    },
    FilterInserted {
        status: u32,
        slot: i32,
        properties: Option<Properties>,
    },
    FilterInfo {
        status: u32,
        slot: i32,
        properties: Option<Properties>,
    },
    FilterRemoved(u32),
    FiltersMoved(u32),
    AttributeSet(u32),
    FilterReset {
        slot: i32,
        status: u32,
    },
    StackReset(u32),
    FovRange {
        minimum: f32,
        maximum: f32,
    },
    Fov(f32),
    FovSet(u32),
    RollRange {
        minimum: f32,
        maximum: f32,
    },
    RollSet(u32),
    ShotPermissions {
        integration: bool,
        hdr: bool,
        shots: Vec<bool>,
    },
    HighresList(Vec<Highres>),
    PanoramaRange {
        minimum: i32,
        maximum: i32,
    },
    CaptureStarted {
        status: u32,
        total: i32,
    },
    CaptureProgress(i32),
    CaptureFinished(u32),
    CaptureProcessed {
        status: u32,
        path: String,
    },
    CaptureAborted(u32),
    Estimate {
        width: u64,
        height: u64,
        megapixels: u64,
        bytes: u64,
    },
    ScreenResolution {
        status: u32,
        width: i32,
        height: i32,
    },
    GridOfThirds(u32),
    HighQuality(u32),
    Language(u32),
    Settings(Vec<(String, String)>),
    Error {
        fatal: bool,
        code: u32,
        reason: String,
    },
    LogFile(String),
    Input(u32),
    Other(u32),
}

fn texts(body: &[u8], field: u32) -> Vec<String> {
    wire::fields(body)
        .filter(|(number, _)| *number == field)
        .filter_map(|(_, value)| value.text().map(str::to_owned))
        .collect()
}

fn text(body: &[u8], field: u32) -> String {
    wire::find(body, field)
        .and_then(Value::text)
        .unwrap_or_default()
        .to_owned()
}

fn wide(body: &[u8], field: u32) -> u64 {
    wire::find(body, field).and_then(Value::uint).unwrap_or(0)
}

fn number(body: &[u8], field: u32) -> u32 {
    wide(body, field) as u32
}

fn integer(body: &[u8], field: u32) -> i32 {
    wide(body, field) as i32
}

fn flag(body: &[u8], field: u32) -> bool {
    wide(body, field) != 0
}

fn float(body: &[u8], field: u32) -> f32 {
    wire::find(body, field)
        .and_then(Value::float)
        .unwrap_or(0.0)
}

fn flags(body: &[u8], field: u32) -> Vec<bool> {
    wire::fields(body)
        .filter(|(number, _)| *number == field)
        .flat_map(|(_, value)| match value {
            Value::Bytes(packed) => packed.iter().map(|byte| *byte != 0).collect(),
            other => vec![other.uint().unwrap_or(0) != 0],
        })
        .collect()
}

fn messages(body: &[u8], field: u32) -> impl Iterator<Item = &[u8]> {
    wire::fields(body)
        .filter(move |(number, _)| *number == field)
        .filter_map(|(_, value)| value.bytes())
}

fn component(body: &[u8], scalar: Scalar) -> Component {
    let read = |field: u32| match scalar {
        Scalar::Float => float(body, field),
        Scalar::Bool => f32::from(u8::from(flag(body, field))),
        Scalar::Int => integer(body, field) as f32,
        Scalar::Uint => number(body, field) as f32,
    };
    let (step, name) = match scalar {
        Scalar::Bool => (1.0, text(body, 5)),
        _ => (read(5), text(body, 9)),
    };
    Component {
        current: read(1),
        default: read(2),
        minimum: read(3),
        maximum: read(4),
        step,
        name,
    }
}

fn control(body: &[u8]) -> Control {
    let (scalar, components) = [
        (Scalar::Float, 6),
        (Scalar::Bool, 7),
        (Scalar::Int, 8),
        (Scalar::Uint, 9),
    ]
    .into_iter()
    .map(|(scalar, field)| {
        let list: Vec<_> = messages(body, field)
            .map(|values| component(values, scalar))
            .collect();
        (scalar, list)
    })
    .find(|(_, list)| !list.is_empty())
    .unwrap_or((Scalar::Float, Vec::new()));
    Control {
        id: integer(body, 1),
        name: text(body, 2),
        kind: ControlKind::from_code(number(body, 5)),
        scalar,
        minimum_shown: float(body, 3),
        maximum_shown: float(body, 4),
        precision: integer(body, 10),
        unit: text(body, 11),
        labels: texts(body, 12),
        tooltip: text(body, 13),
        components,
    }
}

fn properties(body: &[u8], field: u32) -> Option<Properties> {
    let body = wire::find(body, field)?.bytes()?;
    Some(Properties {
        id: text(body, 1),
        name: text(body, 2),
        controls: messages(body, 3).map(control).collect(),
    })
}

impl Response {
    pub fn decode(message: &[u8]) -> Option<Self> {
        let response = wire::find(message, RESPONSE)?.bytes()?;
        let (field, value) = wire::fields(response).next()?;
        let body = value.bytes()?;
        Some(match field {
            1 => {
                let ids = texts(body, 1);
                let names = texts(body, 2);
                let filters = ids
                    .into_iter()
                    .enumerate()
                    .map(|(index, id)| Filter {
                        name: names.get(index).cloned().unwrap_or_else(|| id.clone()),
                        id,
                    })
                    .collect();
                Self::Filters(filters)
            }
            2 => Self::HighresList(
                messages(body, 1)
                    .map(|entry| Highres {
                        multiplier: integer(entry, 1),
                        width: integer(entry, 2),
                        height: integer(entry, 3),
                    })
                    .collect(),
            ),
            3 => Self::CaptureStarted {
                status: number(body, 1),
                total: integer(body, 3),
            },
            4 => Self::CaptureFinished(number(body, 1)),
            5 => Self::CaptureProcessed {
                status: number(body, 1),
                path: text(body, 2),
            },
            6 => Self::PanoramaRange {
                minimum: integer(body, 3),
                maximum: integer(body, 4),
            },
            7 => Self::FovRange {
                minimum: float(body, 1),
                maximum: float(body, 2),
            },
            8 | 58 => Self::FovSet(number(body, 1)),
            9 => Self::RollRange {
                minimum: float(body, 1),
                maximum: float(body, 2),
            },
            10 | 56 => Self::RollSet(number(body, 1)),
            11 => Self::Enabled(number(body, 1)),
            12 => Self::CaptureAborted(number(body, 1)),
            13 => Self::IsEnabled(flag(body, 1)),
            14 => Self::Fov(float(body, 1)),
            15 => Self::CaptureProgress(integer(body, 1)),
            16 => Self::LogFile(text(body, 1)),
            17 => Self::FilterSet {
                status: number(body, 1),
                slot: integer(body, 2),
                properties: properties(body, 3),
            },
            18 => Self::AttributeSet(number(body, 1)),
            19 => Self::Available(flag(body, 1)),
            21 => Self::ShotPermissions {
                integration: flag(body, 1),
                hdr: flag(body, 2),
                shots: flags(body, 3),
            },
            22 => Self::Input(number(body, 1)),
            23 => Self::Language(number(body, 1)),
            24 => Self::FilterInfo {
                status: number(body, 1),
                slot: integer(body, 2),
                properties: properties(body, 3),
            },
            25 => Self::FilterReset {
                slot: integer(body, 1),
                status: number(body, 2),
            },
            27 => Self::Version(number(body, 1), number(body, 2), number(body, 3)),
            29 => Self::UiReady,
            34 => Self::Process {
                id: number(body, 2),
                path: text(body, 3),
            },
            35 => Self::ScreenResolution {
                status: number(body, 1),
                width: integer(body, 2),
                height: integer(body, 3),
            },
            37 => Self::GridOfThirds(number(body, 1)),
            38 => Self::StackReset(number(body, 1)),
            39 => Self::Features {
                sdk: flag(body, 1),
                filters: flag(body, 2),
                restyle: flag(body, 3),
            },
            45 => Self::FiltersMoved(number(body, 1)),
            46 => Self::Stack(texts(body, 1)),
            47 => Self::FilterInserted {
                status: number(body, 1),
                slot: integer(body, 2),
                properties: properties(body, 3),
            },
            48 => Self::FilterRemoved(number(body, 1)),
            49 => Self::Error {
                fatal: number(body, 1) == 1,
                code: number(body, 2),
                reason: text(body, 5),
            },
            50 => Self::Estimate {
                width: wide(body, 5),
                height: wide(body, 6),
                megapixels: wide(body, 7),
                bytes: wide(body, 8),
            },
            52 => Self::Settings(
                messages(body, 1)
                    .map(|entry| (text(entry, 1), text(entry, 2)))
                    .collect(),
            ),
            57 => Self::EnabledFeatures {
                filters: flag(body, 1),
            },
            60 => Self::HighQuality(number(body, 1)),
            other => Self::Other(other),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GameControl {
    Button,
    Slider {
        minimum: f32,
        maximum: f32,
        value: f32,
        step: f32,
    },
    Toggle(bool),
    List {
        entries: Vec<(i32, String)>,
        selected: i32,
    },
    Edit(String),
    Label,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Ready {
        counter: u32,
    },
    Status {
        status: u32,
    },
    AddControl {
        request: i32,
        id: i32,
        name: String,
        visible: bool,
        control: GameControl,
    },
    RemoveControls(Vec<i32>),
    RemoveAllControls,
    SetVisible {
        id: i32,
        visible: bool,
    },
    Other(u32),
}

fn game_control(body: &[u8]) -> Option<(i32, String, GameControl)> {
    let (field, value) = wire::fields(body).find(|(field, _)| (4..=9).contains(field))?;
    let described = value.bytes()?;
    let control = match field {
        4 => GameControl::Button,
        5 => GameControl::Slider {
            minimum: float(described, 3),
            maximum: float(described, 4),
            value: float(described, 5),
            step: float(described, 6),
        },
        6 => GameControl::Edit(text(described, 4)),
        7 => GameControl::Toggle(flag(described, 3)),
        8 => GameControl::Label,
        _ => GameControl::List {
            entries: messages(described, 3)
                .map(|entry| (integer(entry, 1), text(entry, 2)))
                .collect(),
            selected: integer(described, 4),
        },
    };
    Some((integer(described, 1), text(described, 2), control))
}

impl Incoming {
    pub fn decode(message: &[u8]) -> Option<Self> {
        let request = wire::find(message, REQUEST)?.bytes()?;
        let (field, value) = wire::fields(request).next()?;
        let body = value.bytes()?;
        Some(match field {
            58 => Self::Ready {
                counter: number(body, 1),
            },
            51 => Self::Status {
                status: number(body, 1),
            },
            27 => match game_control(body) {
                Some((id, name, control)) => Self::AddControl {
                    request: integer(body, 1),
                    id,
                    name,
                    visible: wire::find(body, 3).is_none() || flag(body, 3),
                    control,
                },
                None => Self::Other(field),
            },
            30 => Self::RemoveControls(
                wire::fields(body)
                    .filter(|(number, _)| *number == 1)
                    .filter_map(|(_, value)| value.uint())
                    .map(|id| id as i32)
                    .collect(),
            ),
            49 => Self::RemoveAllControls,
            31 => Self::SetVisible {
                id: integer(body, 1),
                visible: flag(body, 2),
            },
            other => Self::Other(other),
        })
    }

    pub fn reply(&self) -> Option<Vec<u8>> {
        let ok = u64::from(status::OK);
        let (field, body) = match self {
            Self::Ready { .. } => (59, Writer::new().uint(1, ok)),
            Self::Status { .. } => (53, Writer::new().uint(1, ok)),
            Self::AddControl { request, .. } => {
                (28, Writer::new().uint(1, signed(*request)).uint(2, ok))
            }
            Self::RemoveControls(_) => (31, Writer::new().uint(1, ok)),
            Self::RemoveAllControls => (51, Writer::new().uint(1, ok)),
            Self::SetVisible { .. } => (32, Writer::new().uint(1, ok)),
            Self::Other(_) => return None,
        };
        let reply = Writer::new().message(field, body);
        Some(Writer::new().message(RESPONSE, reply).finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(field: u32, body: Writer) -> Vec<u8> {
        Writer::new()
            .message(RESPONSE, Writer::new().message(field, body))
            .finish()
    }

    fn sent(request: &Request) -> (u32, Vec<u8>) {
        let encoded = request.encode();
        let inner = wire::find(&encoded, REQUEST).unwrap().bytes().unwrap();
        let (field, body) = wire::fields(inner).next().unwrap();
        (field, body.bytes().unwrap().to_vec())
    }

    #[test]
    fn the_ready_message_the_game_sent_is_understood_and_answered() {
        let heard = [0x0a, 0x05, 0xd2, 0x03, 0x02, 0x08, 0x01];
        let incoming = Incoming::decode(&heard).unwrap();
        assert_eq!(incoming, Incoming::Ready { counter: 1 });
        let answer = incoming.reply().unwrap();
        let response = wire::find(&answer, RESPONSE).unwrap().bytes().unwrap();
        let body = wire::find(response, 59).unwrap().bytes().unwrap();
        assert_eq!(number(body, 1), status::OK);
        assert_eq!(Response::decode(&heard), None);
        assert_eq!(Incoming::decode(&answer), None);
    }

    #[test]
    fn a_request_is_wrapped_as_the_request_side_of_the_ipc_message() {
        let request = Request::SetFilter {
            filter: "Vignette.fx".to_owned(),
            slot: 2,
        };
        let (field, body) = sent(&request);
        assert_eq!(field, 13);
        assert_eq!(text(&body, 1), "Vignette.fx");
        assert_eq!(integer(&body, 2), 2);
        assert!(wire::find(&request.encode(), RESPONSE).is_none());
    }

    #[test]
    fn a_request_without_arguments_still_names_itself() {
        assert_eq!(sent(&Request::Filters), (3, Vec::new()));
        assert_eq!(sent(&Request::Stack), (47, Vec::new()));
    }

    #[test]
    fn the_filter_session_starts_without_pausing_the_game() {
        let (field, body) = sent(&Request::Enable {
            on: true,
            keep_filters: true,
            pause: false,
        });
        assert_eq!(field, 1);
        assert_eq!(
            (number(&body, 1), number(&body, 2), number(&body, 3)),
            VERSION
        );
        assert!(flag(&body, 4));
        assert!(flag(&body, 5));
        assert!(!flag(&body, 6));
    }

    #[test]
    fn a_negative_slot_is_written_the_way_protobuf_writes_int32() {
        let (_, body) = sent(&Request::RemoveFilter { slot: -1 });
        assert_eq!(wide(&body, 1), u64::MAX);
        assert_eq!(integer(&body, 1), -1);
    }

    #[test]
    fn every_component_of_a_slider_is_sent_with_its_own_type() {
        let (field, body) = sent(&Request::SetAttribute {
            filter: "Color.fx".to_owned(),
            slot: 0,
            control: 3,
            values: Values::Floats(vec![0.25, 0.5, 1.0]),
        });
        assert_eq!(field, 14);
        let floats: Vec<f32> = wire::fields(&body)
            .filter(|(number, _)| *number == 4)
            .filter_map(|(_, value)| value.float())
            .collect();
        assert_eq!(floats, [0.25, 0.5, 1.0]);
        let (_, toggled) = sent(&Request::SetAttribute {
            filter: "Details.fx".to_owned(),
            slot: 1,
            control: 0,
            values: Values::Bools(vec![true]),
        });
        assert!(flag(&toggled, 5));
    }

    #[test]
    fn a_new_order_lists_every_slot() {
        let (field, body) = sent(&Request::MoveFilters {
            order: vec![2, 0, 1],
        });
        assert_eq!(field, 46);
        let order: Vec<u64> = wire::fields(&body)
            .filter_map(|(_, value)| value.uint())
            .collect();
        assert_eq!(order, [2, 0, 1]);
    }

    #[test]
    fn a_super_resolution_shot_carries_its_multiplier() {
        let capture = Capture {
            shot: Shot::Highres,
            multiplier: Some(4),
            enhance: true,
            ..Capture::regular()
        };
        let (field, body) = sent(&Request::Capture(capture));
        assert_eq!(field, 2);
        assert_eq!(number(&body, 1), 2);
        assert_eq!(integer(&body, 2), 4);
        assert!(flag(&body, 6));
        assert!(wire::find(&body, 3).is_none());
        let (estimate, estimated) = sent(&Request::Estimate(capture));
        assert_eq!(estimate, 48);
        assert!(wire::find(&estimated, 7).is_none());
    }

    #[test]
    fn the_filter_list_pairs_each_id_with_its_name() {
        let body = Writer::new()
            .text(1, "Adjustments.fx")
            .text(1, "NvVignette.fx")
            .text(2, "Brightness / Contrast")
            .text(2, "Vignette");
        assert_eq!(
            Response::decode(&reply(1, body)),
            Some(Response::Filters(vec![
                Filter {
                    id: "Adjustments.fx".to_owned(),
                    name: "Brightness / Contrast".to_owned(),
                },
                Filter {
                    id: "NvVignette.fx".to_owned(),
                    name: "Vignette".to_owned(),
                },
            ]))
        );
    }

    #[test]
    fn a_filter_without_a_name_is_shown_by_its_id() {
        assert_eq!(
            Response::decode(&reply(1, Writer::new().text(1, "Sharpen.fx"))),
            Some(Response::Filters(vec![Filter {
                id: "Sharpen.fx".to_owned(),
                name: "Sharpen.fx".to_owned(),
            }]))
        );
    }

    fn slider() -> Writer {
        let values = Writer::new()
            .float(1, 0.3)
            .float(2, 0.0)
            .float(3, -1.0)
            .float(4, 1.0)
            .float(5, 0.01)
            .text(9, "x");
        Writer::new()
            .uint(1, 7)
            .text(2, "Exposure")
            .float(3, -100.0)
            .float(4, 100.0)
            .uint(5, 1)
            .message(6, values)
            .uint(10, 0)
            .text(11, "%")
    }

    #[test]
    fn an_applied_filter_comes_back_with_its_sliders() {
        let properties = Writer::new()
            .text(1, "Adjustments.fx")
            .text(2, "Brightness / Contrast")
            .message(3, slider());
        let body = Writer::new().uint(1, 1).uint(2, 0).message(3, properties);
        let Some(Response::FilterSet {
            status,
            slot,
            properties: Some(properties),
        }) = Response::decode(&reply(17, body))
        else {
            panic!("the reply was not read as an applied filter");
        };
        assert!(status::succeeded(status));
        assert_eq!(slot, 0);
        assert_eq!(properties.name, "Brightness / Contrast");
        let control = &properties.controls[0];
        assert_eq!((control.id, control.name.as_str()), (7, "Exposure"));
        assert_eq!(control.kind, ControlKind::Slider);
        assert_eq!(control.scalar, Scalar::Float);
        assert_eq!(
            (control.minimum_shown, control.maximum_shown),
            (-100.0, 100.0)
        );
        assert_eq!(control.unit, "%");
        assert_eq!(
            control.components,
            [Component {
                current: 0.3,
                default: 0.0,
                minimum: -1.0,
                maximum: 1.0,
                step: 0.01,
                name: "x".to_owned(),
            }]
        );
        assert_eq!(control.values(), Values::Floats(vec![0.3]));
    }

    #[test]
    fn a_checkbox_control_keeps_its_boolean_type() {
        let values = Writer::new().flag(1, true).flag(2, false).flag(4, true);
        let described = Writer::new()
            .uint(1, 2)
            .text(2, "Enable Depth")
            .uint(5, 2)
            .message(7, values);
        let parsed = control(&described.finish());
        assert_eq!(parsed.kind, ControlKind::Boolean);
        assert_eq!(parsed.scalar, Scalar::Bool);
        assert_eq!(parsed.values(), Values::Bools(vec![true]));
    }

    #[test]
    fn shot_permissions_are_read_packed_or_one_by_one() {
        let single = Writer::new()
            .flag(1, true)
            .flag(2, false)
            .flag(3, true)
            .flag(3, false)
            .flag(3, true);
        let packed = Writer::new().flag(1, true).bytes(3, &[1, 0, 1]);
        let expected = Some(Response::ShotPermissions {
            integration: true,
            hdr: false,
            shots: vec![true, false, true],
        });
        assert_eq!(Response::decode(&reply(21, single)), expected);
        assert_eq!(Response::decode(&reply(21, packed)), expected);
    }

    #[test]
    fn capture_progress_ends_with_the_saved_file() {
        assert_eq!(
            Response::decode(&reply(3, Writer::new().uint(1, 3).uint(3, 12))),
            Some(Response::CaptureStarted {
                status: status::STARTED,
                total: 12
            })
        );
        assert_eq!(
            Response::decode(&reply(15, Writer::new().uint(1, 5))),
            Some(Response::CaptureProgress(5))
        );
        assert_eq!(
            Response::decode(&reply(
                5,
                Writer::new().uint(1, 28).text(2, "C:\\Videos\\shot.png")
            )),
            Some(Response::CaptureProcessed {
                status: status::SHOT_SAVED,
                path: "C:\\Videos\\shot.png".to_owned()
            })
        );
    }

    #[test]
    fn replies_carry_their_status_and_unknown_ones_keep_their_number() {
        assert_eq!(
            Response::decode(&reply(19, Writer::new().flag(1, true))),
            Some(Response::Available(true))
        );
        assert_eq!(
            Response::decode(&reply(27, Writer::new().uint(1, 7).uint(2, 9).uint(3, 0))),
            Some(Response::Version(7, 9, 0))
        );
        assert_eq!(
            Response::decode(&reply(11, Writer::new().uint(1, 17))),
            Some(Response::Enabled(status::OK_FILTERS_ONLY))
        );
        assert_eq!(
            Response::decode(&reply(
                39,
                Writer::new().flag(1, false).flag(2, true).flag(3, false)
            )),
            Some(Response::Features {
                sdk: false,
                filters: true,
                restyle: false
            })
        );
        assert_eq!(
            Response::decode(&reply(44, Writer::new())),
            Some(Response::Other(44))
        );
        assert_eq!(Response::decode(&Request::Filters.encode()), None);
        assert!(status::succeeded(status::OK_FILTERS_ONLY));
        assert!(!status::succeeded(status::NEEDS_DEPTH));
    }

    #[test]
    fn a_control_the_game_adds_is_acknowledged_by_its_request_number() {
        let described = Writer::new()
            .uint(1, 40)
            .text(2, "Time of day")
            .float(3, 0.0)
            .float(4, 24.0)
            .float(5, 12.0)
            .float(6, 0.5);
        let request = Writer::new()
            .uint(1, 9)
            .uint(2, 1)
            .flag(3, true)
            .message(5, described);
        let message = Writer::new()
            .message(REQUEST, Writer::new().message(27, request))
            .finish();
        let incoming = Incoming::decode(&message).unwrap();
        assert_eq!(
            incoming,
            Incoming::AddControl {
                request: 9,
                id: 40,
                name: "Time of day".to_owned(),
                visible: true,
                control: GameControl::Slider {
                    minimum: 0.0,
                    maximum: 24.0,
                    value: 12.0,
                    step: 0.5
                },
            }
        );
        let answer = incoming.reply().unwrap();
        let response = wire::find(&answer, RESPONSE).unwrap().bytes().unwrap();
        let body = wire::find(response, 28).unwrap().bytes().unwrap();
        assert_eq!((integer(body, 1), number(body, 2)), (9, status::OK));
        let (field, changed) = sent(&Request::ControlChanged {
            control: 40,
            change: ControlChange::Slider(18.0),
        });
        assert_eq!(field, 29);
        let slider = wire::find(&changed, 4).unwrap().bytes().unwrap();
        assert_eq!((integer(slider, 1), float(slider, 5)), (40, 18.0));
    }
}
