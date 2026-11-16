use std::collections::VecDeque;

use crate::ansel::{
    Capture, Control, ControlChange, Filter, GameControl, Highres, Incoming, NO_FILTER, Request,
    Response, Scalar, Values, status,
};

const LANGUAGE: i32 = 0x19;
const SUBLANGUAGE: i32 = 0x01;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    Filters,
    Photo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    pub photo: bool,
    pub filters: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub slot: i32,
    pub id: String,
    pub name: String,
    pub loaded: bool,
    pub controls: Vec<Control>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub minimum: f32,
    pub maximum: f32,
    pub value: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Shooting {
    #[default]
    Idle,
    Running {
        done: i32,
        total: i32,
    },
    Saved(String),
    Failed(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameEntry {
    pub id: i32,
    pub name: String,
    pub visible: bool,
    pub control: GameControl,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StyleLayer {
    pub filter: String,
    pub values: Vec<(i32, Vec<f32>)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub present: bool,
    pub process: u32,
    pub path: String,
    pub features: Option<Features>,
    pub mode: Mode,
    pub filters: Vec<Filter>,
    pub layers: Vec<Layer>,
    pub fov: Option<Span>,
    pub roll: Option<Span>,
    pub highres: Vec<Highres>,
    pub panorama: Option<(i32, i32)>,
    pub shots: Vec<bool>,
    pub hdr: bool,
    pub shooting: Shooting,
    pub game_controls: Vec<GameEntry>,
    pub restoring: bool,
    pub error: Option<String>,
}

impl State {
    pub fn style(&self) -> Vec<StyleLayer> {
        self.layers
            .iter()
            .map(|layer| StyleLayer {
                filter: file_name(&layer.id).to_owned(),
                values: layer
                    .controls
                    .iter()
                    .map(|control| {
                        let values = control.components.iter().map(|part| part.current);
                        (control.id, values.collect())
                    })
                    .collect(),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Open(Mode),
    Close,
    Add(String),
    Remove(usize),
    Move {
        from: usize,
        to: usize,
    },
    Set {
        layer: usize,
        control: i32,
        component: usize,
        value: f32,
    },
    ResetLayer(usize),
    Clear,
    Apply(Vec<StyleLayer>),
    Fov(f32),
    Roll(f32),
    Capture(Capture),
    Abort,
    Grid(bool),
    Game {
        control: i32,
        change: ControlChange,
    },
    Input {
        message: i32,
        wparam: i32,
        lparam: i32,
        delta: bool,
    },
}

impl Command {
    pub fn replaces(&self, earlier: &Self) -> bool {
        match (self, earlier) {
            (Self::Fov(_), Self::Fov(_)) | (Self::Roll(_), Self::Roll(_)) => true,
            (
                Self::Set {
                    layer,
                    control,
                    component,
                    ..
                },
                Self::Set {
                    layer: other_layer,
                    control: other_control,
                    component: other_component,
                    ..
                },
            ) => layer == other_layer && control == other_control && component == other_component,
            _ => false,
        }
    }
}

pub fn file_name(id: &str) -> &str {
    id.rsplit(['\\', '/']).next().unwrap_or(id)
}

fn values_of(control: &Control, numbers: &[f32]) -> Values {
    match control.scalar {
        Scalar::Float => Values::Floats(numbers.to_vec()),
        Scalar::Bool => Values::Bools(numbers.iter().map(|value| *value != 0.0).collect()),
        Scalar::Int => Values::Ints(numbers.iter().map(|value| value.round() as i32).collect()),
        Scalar::Uint => Values::Uints(
            numbers
                .iter()
                .map(|value| value.round().max(0.0) as u32)
                .collect(),
        ),
    }
}

#[derive(Debug, Default)]
pub struct Machine {
    state: State,
    wanted: Option<Mode>,
    restoring: Option<VecDeque<StyleLayer>>,
    restored: i32,
    close_after: bool,
}

impl Machine {
    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn seen(&mut self, present: bool, process: u32) -> Vec<Request> {
        if !present {
            *self = Self::default();
            return Vec::new();
        }
        if process != 0 {
            self.state.process = process;
        }
        if self.state.present {
            return Vec::new();
        }
        self.state.present = true;
        vec![
            Request::UiReady,
            Request::Language {
                language: LANGUAGE,
                sublanguage: SUBLANGUAGE,
            },
            Request::Features,
            Request::Process,
        ]
    }

    pub fn incoming(&mut self, incoming: &Incoming) -> Vec<Request> {
        match incoming {
            Incoming::Ready { .. } => {
                let known = self.state.present;
                self.state = State {
                    present: known,
                    process: self.state.process,
                    ..State::default()
                };
                self.wanted = None;
                self.restoring = None;
                vec![Request::Features, Request::Process]
            }
            Incoming::AddControl {
                id,
                name,
                visible,
                control,
                ..
            } => {
                self.state.game_controls.retain(|entry| entry.id != *id);
                self.state.game_controls.push(GameEntry {
                    id: *id,
                    name: name.clone(),
                    visible: *visible,
                    control: control.clone(),
                });
                Vec::new()
            }
            Incoming::RemoveControls(ids) => {
                self.state
                    .game_controls
                    .retain(|entry| !ids.contains(&entry.id));
                Vec::new()
            }
            Incoming::RemoveAllControls => {
                self.state.game_controls.clear();
                Vec::new()
            }
            Incoming::SetVisible { id, visible } => {
                for entry in &mut self.state.game_controls {
                    if entry.id == *id {
                        entry.visible = *visible;
                    }
                }
                Vec::new()
            }
            Incoming::Status { .. } | Incoming::Other(_) => Vec::new(),
        }
    }

    fn open(&mut self, mode: Mode) -> Vec<Request> {
        self.wanted = Some(mode);
        vec![Request::Enable {
            on: mode != Mode::Off,
            keep_filters: true,
            pause: mode == Mode::Photo,
        }]
    }

    fn slot_for_new(&self) -> i32 {
        self.state
            .layers
            .iter()
            .map(|layer| layer.slot + 1)
            .max()
            .unwrap_or(0)
    }

    fn restore_next(&mut self) -> Vec<Request> {
        let Some(queue) = &mut self.restoring else {
            return Vec::new();
        };
        while let Some(layer) = queue.front() {
            let found = self
                .state
                .filters
                .iter()
                .find(|filter| file_name(&filter.id).eq_ignore_ascii_case(&layer.filter));
            match found {
                Some(filter) => {
                    return vec![Request::SetFilter {
                        filter: filter.id.clone(),
                        slot: self.restored,
                    }];
                }
                None => {
                    queue.pop_front();
                }
            }
        }
        self.restoring = None;
        self.state.restoring = false;
        let mut requests = vec![Request::Stack];
        if std::mem::take(&mut self.close_after) {
            requests.extend(self.open(Mode::Off));
        }
        requests
    }

    fn restored_layer(&mut self, properties: Option<&crate::ansel::Properties>) -> Vec<Request> {
        let Some(saved) = self.restoring.as_mut().and_then(VecDeque::pop_front) else {
            return vec![Request::Stack];
        };
        let slot = self.restored;
        self.restored += 1;
        let mut requests = Vec::new();
        if let Some(properties) = properties {
            for (id, numbers) in &saved.values {
                let Some(control) = properties.controls.iter().find(|control| control.id == *id)
                else {
                    continue;
                };
                let current: Vec<f32> =
                    control.components.iter().map(|part| part.current).collect();
                if numbers.len() == current.len() && *numbers != current {
                    requests.push(Request::SetAttribute {
                        filter: properties.id.clone(),
                        slot,
                        control: *id,
                        values: values_of(control, numbers),
                    });
                }
            }
        }
        requests.extend(self.restore_next());
        requests
    }

    pub fn command(&mut self, command: Command) -> Vec<Request> {
        if !self.state.present {
            return Vec::new();
        }
        match command {
            Command::Open(mode) => self.open(mode),
            Command::Close => self.open(Mode::Off),
            Command::Add(filter) => vec![Request::SetFilter {
                filter,
                slot: self.slot_for_new(),
            }],
            Command::Remove(index) => self
                .state
                .layers
                .get(index)
                .map(|layer| vec![Request::RemoveFilter { slot: layer.slot }])
                .unwrap_or_default(),
            Command::Move { from, to } => {
                let count = self.state.layers.len();
                if from >= count || to >= count || from == to {
                    return Vec::new();
                }
                let mut order: Vec<u32> = (0..count as u32).collect();
                let moved = order.remove(from);
                order.insert(to, moved);
                vec![Request::MoveFilters { order }]
            }
            Command::Set {
                layer,
                control,
                component,
                value,
            } => {
                let Some(layer) = self.state.layers.get_mut(layer) else {
                    return Vec::new();
                };
                let Some(found) = layer.controls.iter_mut().find(|entry| entry.id == control)
                else {
                    return Vec::new();
                };
                let Some(part) = found.components.get_mut(component) else {
                    return Vec::new();
                };
                part.current = value.clamp(part.minimum, part.maximum);
                vec![Request::SetAttribute {
                    filter: layer.id.clone(),
                    slot: layer.slot,
                    control,
                    values: found.values(),
                }]
            }
            Command::ResetLayer(index) => self
                .state
                .layers
                .get(index)
                .map(|layer| vec![Request::ResetFilter { slot: layer.slot }])
                .unwrap_or_default(),
            Command::Clear => vec![Request::ResetStack],
            Command::Apply(style) => {
                self.restoring = Some(style.into());
                self.state.restoring = true;
                self.restored = 0;
                if self.state.mode == Mode::Off {
                    self.close_after = true;
                    self.open(Mode::Filters)
                } else if self.state.filters.is_empty() {
                    vec![Request::Filters]
                } else {
                    vec![Request::ResetStack]
                }
            }
            Command::Fov(value) => {
                if let Some(span) = &mut self.state.fov {
                    span.value = value.clamp(span.minimum, span.maximum);
                }
                vec![Request::SetFov(value)]
            }
            Command::Roll(value) => {
                if let Some(span) = &mut self.state.roll {
                    span.value = value.clamp(span.minimum, span.maximum);
                }
                vec![Request::SetRoll(value)]
            }
            Command::Capture(capture) => {
                self.state.shooting = Shooting::Running { done: 0, total: 0 };
                vec![Request::Capture(capture)]
            }
            Command::Abort => vec![Request::AbortCapture],
            Command::Grid(on) => vec![Request::GridOfThirds(on)],
            Command::Input {
                message,
                wparam,
                lparam,
                delta,
            } => {
                if self.state.mode != Mode::Photo {
                    return Vec::new();
                }
                vec![Request::Input {
                    message,
                    wparam,
                    lparam,
                    delta,
                }]
            }
            Command::Game { control, change } => {
                for entry in &mut self.state.game_controls {
                    if entry.id != control {
                        continue;
                    }
                    match (&mut entry.control, &change) {
                        (GameControl::Slider { value, .. }, ControlChange::Slider(new)) => {
                            *value = *new;
                        }
                        (GameControl::Toggle(on), ControlChange::Toggle(new)) => *on = *new,
                        (GameControl::List { selected, .. }, ControlChange::Selected(new)) => {
                            *selected = *new;
                        }
                        (GameControl::Edit(text), ControlChange::Text(new)) => {
                            text.clone_from(new);
                        }
                        _ => {}
                    }
                }
                vec![Request::ControlChanged { control, change }]
            }
        }
    }

    fn fail(&mut self, code: u32) {
        if !status::succeeded(code) {
            self.state.error = Some(format!("код {code}"));
        }
    }

    pub fn response(&mut self, response: Response) -> Vec<Request> {
        match response {
            Response::Features { sdk, filters, .. } => {
                self.state.features = Some(Features {
                    photo: sdk,
                    filters,
                });
                Vec::new()
            }
            Response::Process { id, path } => {
                if id != 0 {
                    self.state.process = id;
                }
                self.state.path = path;
                Vec::new()
            }
            Response::Enabled(code) => {
                let Some(wanted) = self.wanted.take() else {
                    return Vec::new();
                };
                if !status::succeeded(code) {
                    self.fail(code);
                    self.restoring = None;
                    self.state.restoring = false;
                    self.close_after = false;
                    return Vec::new();
                }
                if wanted == Mode::Off {
                    self.state.mode = Mode::Off;
                    self.state.shooting = Shooting::Idle;
                    return Vec::new();
                }
                self.state.mode = if code == status::OK_PHOTO && wanted == Mode::Photo {
                    Mode::Photo
                } else {
                    Mode::Filters
                };
                self.state.error = None;
                let mut requests = vec![Request::Filters];
                if self.restoring.is_none() {
                    requests.push(Request::Stack);
                }
                if self.state.mode == Mode::Photo {
                    requests.extend([
                        Request::FovRange,
                        Request::Fov,
                        Request::RollRange,
                        Request::ShotPermissions,
                        Request::HighresList,
                        Request::PanoramaRange,
                    ]);
                }
                requests
            }
            Response::Filters(list) => {
                self.state.filters = list;
                if self.restoring.is_some() {
                    vec![Request::ResetStack]
                } else {
                    Vec::new()
                }
            }
            Response::StackReset(code) => {
                self.fail(code);
                if self.restoring.is_some() {
                    self.restored = 0;
                    self.restore_next()
                } else {
                    vec![Request::Stack]
                }
            }
            Response::FilterSet {
                status: code,
                properties,
                ..
            }
            | Response::FilterInserted {
                status: code,
                properties,
                ..
            } => {
                self.fail(code);
                if self.restoring.is_some() {
                    self.restored_layer(properties.as_ref())
                } else {
                    vec![Request::Stack]
                }
            }
            Response::Stack(ids) => {
                let names = &self.state.filters;
                let previous = std::mem::take(&mut self.state.layers);
                self.state.layers = ids
                    .into_iter()
                    .enumerate()
                    .filter(|(_, id)| !id.is_empty() && id != NO_FILTER)
                    .map(|(slot, id)| {
                        let slot = slot as i32;
                        let kept = previous
                            .iter()
                            .find(|layer| layer.slot == slot && layer.id == id);
                        Layer {
                            slot,
                            name: names.iter().find(|filter| filter.id == id).map_or_else(
                                || file_name(&id).to_owned(),
                                |filter| filter.name.clone(),
                            ),
                            loaded: kept.is_some_and(|layer| layer.loaded),
                            controls: kept.map(|layer| layer.controls.clone()).unwrap_or_default(),
                            id,
                        }
                    })
                    .collect();
                self.state
                    .layers
                    .iter()
                    .map(|layer| Request::FilterInfo { slot: layer.slot })
                    .collect()
            }
            Response::FilterInfo {
                slot, properties, ..
            } => {
                if let Some(properties) = properties
                    && let Some(layer) = self
                        .state
                        .layers
                        .iter_mut()
                        .find(|layer| layer.slot == slot)
                {
                    if !properties.name.is_empty() {
                        layer.name = properties.name;
                    }
                    layer.controls = properties.controls;
                    layer.loaded = true;
                }
                Vec::new()
            }
            Response::FilterRemoved(code) | Response::FiltersMoved(code) => {
                self.fail(code);
                vec![Request::Stack]
            }
            Response::FilterReset { slot, status: code } => {
                self.fail(code);
                vec![Request::FilterInfo { slot }]
            }
            Response::AttributeSet(code) | Response::FovSet(code) | Response::RollSet(code) => {
                self.fail(code);
                Vec::new()
            }
            Response::FovRange { minimum, maximum } => {
                let value = self.state.fov.map_or(minimum, |span| span.value);
                self.state.fov = Some(Span {
                    minimum,
                    maximum,
                    value,
                });
                Vec::new()
            }
            Response::Fov(value) => {
                let span = self.state.fov.get_or_insert(Span {
                    minimum: value,
                    maximum: value,
                    value,
                });
                span.value = value;
                Vec::new()
            }
            Response::RollRange { minimum, maximum } => {
                self.state.roll = Some(Span {
                    minimum,
                    maximum,
                    value: 0.0_f32.clamp(minimum, maximum),
                });
                Vec::new()
            }
            Response::ShotPermissions { hdr, shots, .. } => {
                self.state.hdr = hdr;
                self.state.shots = shots;
                Vec::new()
            }
            Response::HighresList(list) => {
                self.state.highres = list;
                Vec::new()
            }
            Response::PanoramaRange { minimum, maximum } => {
                self.state.panorama = Some((minimum, maximum));
                Vec::new()
            }
            Response::CaptureStarted {
                status: code,
                total,
            } => {
                self.state.shooting = if status::succeeded(code) {
                    Shooting::Running { done: 0, total }
                } else {
                    Shooting::Failed(code)
                };
                Vec::new()
            }
            Response::CaptureProgress(done) => {
                if let Shooting::Running { total, .. } = self.state.shooting {
                    self.state.shooting = Shooting::Running { done, total };
                }
                Vec::new()
            }
            Response::CaptureFinished(code) | Response::CaptureAborted(code) => {
                if !status::succeeded(code) {
                    self.state.shooting = Shooting::Failed(code);
                }
                Vec::new()
            }
            Response::CaptureProcessed { status: code, path } => {
                self.state.shooting = if status::succeeded(code) && !path.is_empty() {
                    Shooting::Saved(path)
                } else if status::succeeded(code) {
                    Shooting::Idle
                } else {
                    Shooting::Failed(code)
                };
                Vec::new()
            }
            Response::Error { reason, .. } => {
                self.state.error = Some(reason);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansel::{Component, ControlKind, Properties};

    const VIGNETTE: &str = "C:\\drv\\NvCamera\\NvVignette.fx";
    const SHARPEN: &str = "C:\\drv\\NvCamera\\Sharpen.fx";

    fn filters() -> Vec<Filter> {
        vec![
            Filter {
                id: VIGNETTE.to_owned(),
                name: "Vignette".to_owned(),
            },
            Filter {
                id: SHARPEN.to_owned(),
                name: "Sharpen".to_owned(),
            },
        ]
    }

    fn slider(id: i32, current: f32) -> Control {
        Control {
            id,
            name: "Intensity".to_owned(),
            kind: ControlKind::Slider,
            scalar: Scalar::Float,
            minimum_shown: 0.0,
            maximum_shown: 100.0,
            precision: 0,
            unit: String::new(),
            labels: Vec::new(),
            tooltip: String::new(),
            components: vec![Component {
                current,
                default: 0.5,
                minimum: 0.0,
                maximum: 1.0,
                step: 0.01,
                name: String::new(),
            }],
        }
    }

    fn properties(id: &str, current: f32) -> Properties {
        Properties {
            id: id.to_owned(),
            name: String::new(),
            controls: vec![slider(1, current)],
        }
    }

    fn present() -> Machine {
        let mut machine = Machine::default();
        assert_eq!(
            machine.seen(true, 42),
            [
                Request::UiReady,
                Request::Language {
                    language: LANGUAGE,
                    sublanguage: SUBLANGUAGE
                },
                Request::Features,
                Request::Process
            ]
        );
        machine.response(Response::Features {
            sdk: false,
            filters: true,
            restyle: false,
        });
        machine
    }

    fn opened() -> Machine {
        let mut machine = present();
        machine.command(Command::Open(Mode::Filters));
        machine.response(Response::Enabled(status::OK_FILTERS_ONLY));
        machine.response(Response::Filters(filters()));
        machine
    }

    #[test]
    fn nothing_is_sent_to_a_game_that_is_not_there() {
        let mut machine = Machine::default();
        assert!(machine.command(Command::Open(Mode::Filters)).is_empty());
        assert_eq!(machine.state().mode, Mode::Off);
    }

    #[test]
    fn a_game_is_greeted_once_and_forgotten_when_it_leaves() {
        let mut machine = present();
        assert!(machine.seen(true, 42).is_empty());
        assert_eq!(
            machine.state().features,
            Some(Features {
                photo: false,
                filters: true
            })
        );
        machine.seen(false, 0);
        assert_eq!(*machine.state(), State::default());
    }

    #[test]
    fn opening_filters_does_not_pause_and_then_asks_what_is_applied() {
        let mut machine = present();
        assert_eq!(
            machine.command(Command::Open(Mode::Filters)),
            [Request::Enable {
                on: true,
                keep_filters: true,
                pause: false
            }]
        );
        assert_eq!(
            machine.response(Response::Enabled(status::OK_FILTERS_ONLY)),
            [Request::Filters, Request::Stack]
        );
        assert_eq!(machine.state().mode, Mode::Filters);
    }

    #[test]
    fn photo_mode_falls_back_to_filters_where_the_game_has_no_camera() {
        let mut machine = present();
        assert_eq!(
            machine.command(Command::Open(Mode::Photo)),
            [Request::Enable {
                on: true,
                keep_filters: true,
                pause: true
            }]
        );
        machine.response(Response::Enabled(status::OK_FILTERS_ONLY));
        assert_eq!(machine.state().mode, Mode::Filters);
        let mut photo = present();
        photo.command(Command::Open(Mode::Photo));
        let asked = photo.response(Response::Enabled(status::OK_PHOTO));
        assert_eq!(photo.state().mode, Mode::Photo);
        assert!(asked.contains(&Request::FovRange));
        assert!(asked.contains(&Request::HighresList));
    }

    #[test]
    fn a_refused_session_stays_off_and_says_why() {
        let mut machine = present();
        machine.command(Command::Open(Mode::Filters));
        assert!(machine.response(Response::Enabled(15)).is_empty());
        assert_eq!(machine.state().mode, Mode::Off);
        assert_eq!(machine.state().error.as_deref(), Some("код 15"));
    }

    #[test]
    fn the_stack_skips_empty_slots_and_asks_for_each_filters_sliders() {
        let mut machine = opened();
        let asked = machine.response(Response::Stack(vec![
            NO_FILTER.to_owned(),
            SHARPEN.to_owned(),
        ]));
        assert_eq!(asked, [Request::FilterInfo { slot: 1 }]);
        assert_eq!(machine.state().layers.len(), 1);
        assert_eq!(machine.state().layers[0].name, "Sharpen");
        machine.response(Response::FilterInfo {
            status: status::OK,
            slot: 1,
            properties: Some(properties(SHARPEN, 0.3)),
        });
        assert_eq!(
            machine.state().layers[0].controls[0].components[0].current,
            0.3
        );
    }

    #[test]
    fn a_new_filter_goes_after_the_last_one() {
        let mut machine = opened();
        assert_eq!(
            machine.command(Command::Add(VIGNETTE.to_owned())),
            [Request::SetFilter {
                filter: VIGNETTE.to_owned(),
                slot: 0
            }]
        );
        machine.response(Response::Stack(vec![VIGNETTE.to_owned()]));
        assert_eq!(
            machine.command(Command::Add(SHARPEN.to_owned())),
            [Request::SetFilter {
                filter: SHARPEN.to_owned(),
                slot: 1
            }]
        );
    }

    #[test]
    fn moving_a_slider_sends_the_clamped_value_and_remembers_it() {
        let mut machine = opened();
        machine.response(Response::Stack(vec![VIGNETTE.to_owned()]));
        machine.response(Response::FilterInfo {
            status: status::OK,
            slot: 0,
            properties: Some(properties(VIGNETTE, 0.5)),
        });
        let sent = machine.command(Command::Set {
            layer: 0,
            control: 1,
            component: 0,
            value: 7.0,
        });
        assert_eq!(
            sent,
            [Request::SetAttribute {
                filter: VIGNETTE.to_owned(),
                slot: 0,
                control: 1,
                values: Values::Floats(vec![1.0]),
            }]
        );
        assert_eq!(
            machine.state().style(),
            [StyleLayer {
                filter: "NvVignette.fx".to_owned(),
                values: vec![(1, vec![1.0])],
            }]
        );
    }

    #[test]
    fn reordering_names_the_whole_new_order() {
        let mut machine = opened();
        machine.response(Response::Stack(vec![
            VIGNETTE.to_owned(),
            SHARPEN.to_owned(),
        ]));
        assert_eq!(
            machine.command(Command::Move { from: 1, to: 0 }),
            [Request::MoveFilters { order: vec![1, 0] }]
        );
        assert!(machine.command(Command::Move { from: 0, to: 5 }).is_empty());
        assert_eq!(
            machine.response(Response::FiltersMoved(status::OK)),
            [Request::Stack]
        );
    }

    #[test]
    fn a_saved_style_is_restored_filter_by_filter_with_its_values() {
        let mut machine = opened();
        let style = vec![
            StyleLayer {
                filter: "sharpen.fx".to_owned(),
                values: vec![(1, vec![0.8])],
            },
            StyleLayer {
                filter: "Gone.fx".to_owned(),
                values: Vec::new(),
            },
            StyleLayer {
                filter: "NvVignette.fx".to_owned(),
                values: vec![(1, vec![0.5])],
            },
        ];
        assert_eq!(
            machine.command(Command::Apply(style)),
            [Request::ResetStack]
        );
        assert_eq!(
            machine.response(Response::StackReset(status::OK)),
            [Request::SetFilter {
                filter: SHARPEN.to_owned(),
                slot: 0
            }]
        );
        let next = machine.response(Response::FilterSet {
            status: status::OK,
            slot: 0,
            properties: Some(properties(SHARPEN, 0.5)),
        });
        assert_eq!(
            next,
            [
                Request::SetAttribute {
                    filter: SHARPEN.to_owned(),
                    slot: 0,
                    control: 1,
                    values: Values::Floats(vec![0.8]),
                },
                Request::SetFilter {
                    filter: VIGNETTE.to_owned(),
                    slot: 1
                },
            ]
        );
        let last = machine.response(Response::FilterSet {
            status: status::OK,
            slot: 1,
            properties: Some(properties(VIGNETTE, 0.5)),
        });
        assert_eq!(last, [Request::Stack]);
    }

    #[test]
    fn a_style_applied_from_outside_opens_the_session_and_closes_it_again() {
        let mut machine = present();
        let style = vec![StyleLayer {
            filter: "Sharpen.fx".to_owned(),
            values: Vec::new(),
        }];
        assert_eq!(
            machine.command(Command::Apply(style)),
            [Request::Enable {
                on: true,
                keep_filters: true,
                pause: false
            }]
        );
        assert_eq!(
            machine.response(Response::Enabled(status::OK_FILTERS_ONLY)),
            [Request::Filters]
        );
        assert_eq!(
            machine.response(Response::Filters(filters())),
            [Request::ResetStack]
        );
        machine.response(Response::StackReset(status::OK));
        let closing = machine.response(Response::FilterSet {
            status: status::OK,
            slot: 0,
            properties: Some(properties(SHARPEN, 0.5)),
        });
        assert_eq!(
            closing,
            [
                Request::Stack,
                Request::Enable {
                    on: false,
                    keep_filters: true,
                    pause: false
                }
            ]
        );
        machine.response(Response::Enabled(status::OK));
        assert_eq!(machine.state().mode, Mode::Off);
    }

    #[test]
    fn the_camera_is_steered_only_while_photo_mode_is_on() {
        let steer = Command::Input {
            message: 0x100,
            wparam: 0x57,
            lparam: 0,
            delta: false,
        };
        let mut machine = opened();
        assert!(machine.command(steer.clone()).is_empty());
        let mut photo = present();
        photo.command(Command::Open(Mode::Photo));
        photo.response(Response::Enabled(status::OK_PHOTO));
        assert_eq!(
            photo.command(steer),
            [Request::Input {
                message: 0x100,
                wparam: 0x57,
                lparam: 0,
                delta: false
            }]
        );
    }

    #[test]
    fn a_shot_reports_progress_and_the_saved_file() {
        let mut machine = opened();
        machine.command(Command::Capture(Capture::regular()));
        machine.response(Response::CaptureStarted {
            status: status::STARTED,
            total: 4,
        });
        machine.response(Response::CaptureProgress(3));
        assert_eq!(
            machine.state().shooting,
            Shooting::Running { done: 3, total: 4 }
        );
        machine.response(Response::CaptureProcessed {
            status: status::SHOT_SAVED,
            path: "C:\\shot.png".to_owned(),
        });
        assert_eq!(
            machine.state().shooting,
            Shooting::Saved("C:\\shot.png".to_owned())
        );
    }

    #[test]
    fn a_game_that_restarts_its_camera_is_asked_again_from_scratch() {
        let mut machine = opened();
        machine.response(Response::Stack(vec![VIGNETTE.to_owned()]));
        let asked = machine.incoming(&Incoming::Ready { counter: 2 });
        assert_eq!(asked, [Request::Features, Request::Process]);
        assert!(machine.state().present);
        assert_eq!(machine.state().mode, Mode::Off);
        assert!(machine.state().layers.is_empty());
    }

    #[test]
    fn controls_the_game_adds_are_listed_and_follow_changes() {
        let mut machine = present();
        machine.incoming(&Incoming::AddControl {
            request: 1,
            id: 9,
            name: "Time of day".to_owned(),
            visible: true,
            control: GameControl::Slider {
                minimum: 0.0,
                maximum: 24.0,
                value: 12.0,
                step: 1.0,
            },
        });
        let sent = machine.command(Command::Game {
            control: 9,
            change: ControlChange::Slider(18.0),
        });
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            machine.state().game_controls[0].control,
            GameControl::Slider { value, .. } if value == 18.0
        ));
        machine.incoming(&Incoming::RemoveControls(vec![9]));
        assert!(machine.state().game_controls.is_empty());
    }
}
