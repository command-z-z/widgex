use std::{
    collections::{BTreeMap, HashSet},
    fmt, fs,
    path::{Path, PathBuf},
};

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    pub version: u16,
    #[serde(default)]
    pub theme: Option<Theme>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub sources: Vec<DataSource>,
    #[serde(default)]
    pub windows: Vec<WindowSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Theme {
    #[serde(default)]
    pub css: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct Permissions {
    #[serde(default)]
    pub allow_shell: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DataSource {
    pub id: String,
    pub kind: SourceKind,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Time,
    Battery,
    Cpu,
    Memory,
    Network,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WindowSpec {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default = "default_layer")]
    pub layer: WindowLayer,
    #[serde(default)]
    pub anchor: Vec<AnchorEdge>,
    #[serde(default)]
    pub margin: EdgeInsets,
    #[serde(default)]
    pub size: SizeSpec,
    #[serde(default)]
    pub exclusive_zone: Option<i32>,
    #[serde(default)]
    pub click_through: bool,
    #[serde(default)]
    pub monitor: Option<String>,
    #[serde(default)]
    pub widgets: Vec<WidgetNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowLayer {
    Background,
    Bottom,
    Top,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnchorEdge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct EdgeInsets {
    #[serde(default)]
    pub top: u32,
    #[serde(default)]
    pub right: u32,
    #[serde(default)]
    pub bottom: u32,
    #[serde(default)]
    pub left: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SizeSpec {
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WidgetNode {
    #[serde(rename = "type")]
    pub kind: WidgetKind,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub class: Vec<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default)]
    pub direction: Option<Direction>,
    #[serde(default)]
    pub on_click: Option<Action>,
    #[serde(default)]
    pub children: Vec<WidgetNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    Box,
    Label,
    Button,
    Image,
    Progress,
    Spacer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Row,
    Column,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Command { command: String },
    Emit { event: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub references: Vec<String>,
}

/// A point-in-time reading from a data source, keyed by field name.
///
/// Produced by `widgex-source` pollers and consumed by [`resolve_template`]
/// to substitute `{{ source_id.field }}` bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SourceSnapshot {
    pub id: String,
    pub fields: BTreeMap<String, String>,
}

impl SourceSnapshot {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            fields: BTreeMap::new(),
        }
    }

    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    pub path: String,
    pub message: String,
    pub help: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererPayload {
    pub version: u16,
    pub theme_css: Option<String>,
    pub windows: Vec<RendererWindow>,
    pub sources: Vec<RendererSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererWindow {
    pub id: String,
    pub title: Option<String>,
    pub layer: WindowLayer,
    pub anchor: Vec<AnchorEdge>,
    pub margin: EdgeInsets,
    pub size: SizeSpec,
    pub exclusive_zone: Option<i32>,
    pub click_through: bool,
    pub monitor: Option<String>,
    pub widgets: Vec<RendererWidget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererSource {
    pub id: String,
    pub kind: SourceKind,
    pub interval_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererWidget {
    #[serde(rename = "type")]
    pub kind: WidgetKind,
    pub id: Option<String>,
    pub class: Vec<String>,
    pub text: Option<String>,
    pub value: Option<String>,
    pub src: Option<String>,
    pub direction: Option<Direction>,
    pub bindings: Option<RendererWidgetBindings>,
    pub children: Vec<RendererWidget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RendererWidgetBindings {
    pub text: Vec<String>,
    pub value: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid binding syntax")]
pub struct BindingError;

fn default_layer() -> WindowLayer {
    WindowLayer::Top
}

pub fn parse_config_str(input: &str) -> Result<Config, Vec<ConfigDiagnostic>> {
    toml::from_str(input).map_err(|error| {
        vec![ConfigDiagnostic::new(
            "config",
            format!("failed to parse TOML: {error}"),
            "check the surrounding table names, field names, and value types",
        )]
    })
}

pub fn parse_config_file(path: impl AsRef<Path>) -> Result<Config, Vec<ConfigDiagnostic>> {
    let path = path.as_ref();
    let input = fs::read_to_string(path).map_err(|error| {
        vec![ConfigDiagnostic::new(
            path.display().to_string(),
            format!("failed to read config: {error}"),
            "verify the path exists and is readable",
        )]
    })?;

    parse_config_str(&input)
}

pub fn validate_config(config: &Config) -> Result<(), Vec<ConfigDiagnostic>> {
    let mut diagnostics = Vec::new();

    if config.version != 1 {
        diagnostics.push(ConfigDiagnostic::new(
            "version",
            format!("unsupported config version {}", config.version),
            "set version = 1 for the current Widgex config format",
        ));
    }

    if config.windows.is_empty() {
        diagnostics.push(ConfigDiagnostic::new(
            "windows",
            "config must define at least one window",
            "add a [[windows]] table with an id, size, and widgets",
        ));
    }

    collect_duplicate_ids(
        config.windows.iter().map(|window| window.id.as_str()),
        "window",
        "windows",
        &mut diagnostics,
    );
    collect_duplicate_ids(
        config.sources.iter().map(|source| source.id.as_str()),
        "source",
        "sources",
        &mut diagnostics,
    );

    for (index, source) in config.sources.iter().enumerate() {
        if source.id.trim().is_empty() {
            diagnostics.push(ConfigDiagnostic::new(
                format!("sources[{index}].id"),
                "source id cannot be empty",
                "give every source a stable id, for example id = \"clock\"",
            ));
        }

        if source.kind == SourceKind::Shell {
            if !config.permissions.allow_shell {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("sources[{index}]"),
                    "shell source requires permissions.allow_shell",
                    "add [permissions] allow_shell = true after reviewing the command",
                ));
            }

            if source.command.as_deref().is_none_or(str::is_empty) {
                diagnostics.push(ConfigDiagnostic::new(
                    format!("sources[{index}].command"),
                    "shell source requires a command",
                    "set command = \"your command\"",
                ));
            }
        }
    }

    let source_ids: HashSet<&str> = config
        .sources
        .iter()
        .map(|source| source.id.as_str())
        .collect();

    for (index, window) in config.windows.iter().enumerate() {
        if window.id.trim().is_empty() {
            diagnostics.push(ConfigDiagnostic::new(
                format!("windows[{index}].id"),
                "window id cannot be empty",
                "set a stable window id, for example id = \"top-bar\"",
            ));
        }

        if window.widgets.is_empty() {
            diagnostics.push(ConfigDiagnostic::new(
                format!("windows[{index}].widgets"),
                "window must define at least one widget",
                "add a [[windows.widgets]] entry",
            ));
        }

        for (widget_index, widget) in window.widgets.iter().enumerate() {
            validate_widget_bindings(
                widget,
                &format!("windows[{index}].widgets[{widget_index}]"),
                &source_ids,
                &mut diagnostics,
            );
        }
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

pub fn load_validated_config(path: impl AsRef<Path>) -> Result<Config, Vec<ConfigDiagnostic>> {
    let config = parse_config_file(path)?;
    validate_config(&config)?;
    Ok(config)
}

pub fn renderer_payload_from_config(
    config: &Config,
) -> Result<RendererPayload, Vec<ConfigDiagnostic>> {
    validate_config(config)?;

    Ok(RendererPayload {
        version: config.version,
        theme_css: config.theme.as_ref().and_then(|theme| theme.css.clone()),
        windows: config
            .windows
            .iter()
            .map(|window| RendererWindow {
                id: window.id.clone(),
                title: window.title.clone(),
                layer: window.layer,
                anchor: window.anchor.clone(),
                margin: window.margin,
                size: window.size,
                exclusive_zone: window.exclusive_zone,
                click_through: window.click_through,
                monitor: window.monitor.clone(),
                widgets: window
                    .widgets
                    .iter()
                    .map(renderer_widget_from_config)
                    .collect(),
            })
            .collect(),
        sources: config
            .sources
            .iter()
            .map(|source| RendererSource {
                id: source.id.clone(),
                kind: source.kind,
                interval_ms: source.interval_ms,
            })
            .collect(),
    })
}

pub fn schema_json_pretty<T: JsonSchema>() -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&schema_for!(T))
}

/// Substitute every `{{ source_id.field }}` binding in `template` with the
/// matching value from `snapshots`. Unknown references resolve to an empty
/// string; an unterminated `{{` is emitted verbatim.
pub fn resolve_template(template: &str, snapshots: &[SourceSnapshot]) -> String {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];

        let Some(end) = after.find("}}") else {
            result.push_str(&rest[start..]);
            return result;
        };

        let reference = after[..end].trim();
        result.push_str(&resolve_reference(reference, snapshots).unwrap_or_default());
        rest = &after[end + 2..];
    }

    result.push_str(rest);
    result
}

/// Re-resolve every widget `text`/`value` template in `payload` against the
/// current `snapshots`, returning a payload the renderer can display directly.
pub fn resolve_payload(payload: &RendererPayload, snapshots: &[SourceSnapshot]) -> RendererPayload {
    let mut resolved = payload.clone();
    for window in &mut resolved.windows {
        for widget in &mut window.widgets {
            resolve_widget(widget, snapshots);
        }
    }
    resolved
}

fn resolve_reference(reference: &str, snapshots: &[SourceSnapshot]) -> Option<String> {
    let (source_id, field) = reference.split_once('.')?;
    snapshots
        .iter()
        .find(|snapshot| snapshot.id == source_id)?
        .fields
        .get(field)
        .cloned()
}

fn resolve_widget(widget: &mut RendererWidget, snapshots: &[SourceSnapshot]) {
    if let Some(text) = &widget.text {
        widget.text = Some(resolve_template(text, snapshots));
    }
    if let Some(value) = &widget.value {
        widget.value = Some(resolve_template(value, snapshots));
    }
    for child in &mut widget.children {
        resolve_widget(child, snapshots);
    }
}

pub fn diagnostics_to_string(diagnostics: &[ConfigDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

impl Binding {
    pub fn parse(input: &str) -> Result<Self, BindingError> {
        let mut references = Vec::new();
        let mut rest = input;

        while let Some(start) = rest.find("{{") {
            let after_start = &rest[start + 2..];
            let Some(end) = after_start.find("}}") else {
                return Err(BindingError);
            };
            let reference = after_start[..end].trim();

            if reference.is_empty()
                || !reference
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
            {
                return Err(BindingError);
            }

            references.push(reference.to_string());
            rest = &after_start[end + 2..];
        }

        Ok(Self { references })
    }
}

impl ConfigDiagnostic {
    pub fn new(
        path: impl Into<String>,
        message: impl Into<String>,
        help: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
            help: help.into(),
        }
    }
}

impl fmt::Display for ConfigDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {} (help: {})",
            self.path, self.message, self.help
        )
    }
}

impl std::error::Error for ConfigDiagnostic {}

fn collect_duplicate_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    label: &str,
    path_prefix: &str,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) {
    let mut seen = HashSet::new();

    for (index, id) in ids.enumerate() {
        if !id.is_empty() && !seen.insert(id.to_string()) {
            diagnostics.push(ConfigDiagnostic::new(
                format!("{path_prefix}[{index}].id"),
                format!("duplicate {label} id \"{id}\""),
                format!("make every {label} id unique"),
            ));
        }
    }
}

fn validate_widget_bindings(
    widget: &WidgetNode,
    path: &str,
    source_ids: &HashSet<&str>,
    diagnostics: &mut Vec<ConfigDiagnostic>,
) {
    for (field, value) in [("text", &widget.text), ("value", &widget.value)] {
        let Some(value) = value else {
            continue;
        };

        match Binding::parse(value) {
            Err(_) => diagnostics.push(ConfigDiagnostic::new(
                format!("{path}.{field}"),
                "invalid binding expression",
                "use bindings like {{ source.field }} with a closing }}",
            )),
            Ok(binding) => {
                for reference in &binding.references {
                    let source_id = reference.split('.').next().unwrap_or(reference);
                    if !source_ids.contains(source_id) {
                        diagnostics.push(ConfigDiagnostic::new(
                            format!("{path}.{field}"),
                            format!("binding references unknown source \"{source_id}\""),
                            "add a [[sources]] entry with this id, or fix the reference",
                        ));
                    }
                }
            }
        }
    }

    for (index, child) in widget.children.iter().enumerate() {
        validate_widget_bindings(
            child,
            &format!("{path}.children[{index}]"),
            source_ids,
            diagnostics,
        );
    }
}

fn renderer_widget_from_config(widget: &WidgetNode) -> RendererWidget {
    RendererWidget {
        kind: widget.kind,
        id: widget.id.clone(),
        class: widget.class.clone(),
        text: widget.text.clone(),
        value: widget.value.clone(),
        src: widget.src.clone(),
        direction: widget.direction,
        bindings: renderer_bindings_for_widget(widget),
        children: widget
            .children
            .iter()
            .map(renderer_widget_from_config)
            .collect(),
    }
}

fn renderer_bindings_for_widget(widget: &WidgetNode) -> Option<RendererWidgetBindings> {
    let text = widget
        .text
        .as_deref()
        .and_then(|value| Binding::parse(value).ok())
        .map(|binding| binding.references)
        .unwrap_or_default();
    let value = widget
        .value
        .as_deref()
        .and_then(|value| Binding::parse(value).ok())
        .map(|binding| binding.references)
        .unwrap_or_default();

    if text.is_empty() && value.is_empty() {
        None
    } else {
        Some(RendererWidgetBindings { text, value })
    }
}

pub fn default_config_path() -> PathBuf {
    PathBuf::from("~/.config/widgex/config.toml")
}
