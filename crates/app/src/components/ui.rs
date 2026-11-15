//! Shared building blocks: lucide icons, `.icon-button`, tooltips.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, Hsla, IntoElement, ParentElement, SharedString, Styled, Transformation,
    Window, div, prelude::*, svg,
};
use gpui_component::tooltip::Tooltip;

use crate::theme::{self, sz};

/// A lucide icon at `size` CSS px. `stroke` overrides the default 2.
pub fn icon(name: &str, size: f32) -> Icon {
    icon_with(name, size, None, false)
}

pub fn icon_with(name: &str, size: f32, stroke: Option<f32>, fill: bool) -> Icon {
    let mut path = format!("icons/{name}.svg");
    let mut props = Vec::new();
    if fill {
        props.push("fill".to_string());
    }
    if let Some(width) = stroke {
        props.push(format!("sw={width}"));
    }
    if !props.is_empty() {
        path.push('#');
        path.push_str(&props.join("&"));
    }
    Icon { path: path.into(), size, color: None, transformation: None }
}

/// A lucide icon that, like `currentColor` on the web, takes the inherited
/// text colour. gpui's `svg()` only paints when the svg element itself has a
/// text colour, so the colour is resolved from the text style at render time.
#[derive(IntoElement)]
pub struct Icon {
    path: SharedString,
    size: f32,
    color: Option<Hsla>,
    transformation: Option<Transformation>,
}

impl Icon {

    pub fn with_transformation(mut self, transformation: Transformation) -> Self {
        self.transformation = Some(transformation);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or_else(|| window.text_style().color);
        let mut element = svg().path(self.path).size(sz(self.size)).flex_none().text_color(color);
        if let Some(transformation) = self.transformation {
            element = element.with_transformation(transformation);
        }
        element
    }
}

pub type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// `.icon-button`: 30×30, radius 6, secondary colour, hover surface.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    child: AnyElement,
    label: SharedString,
    width: f32,
    height: f32,
    color: Option<Hsla>,
    active: bool,
    disabled: bool,
    /// `.fullscreen-dialog .icon-button`: no hover background; the icon
    /// brightens to white.
    bare: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            child: child.into_any_element(),
            label: label.into(),
            width: 30.0,
            height: 30.0,
            color: None,
            active: false,
            disabled: false,
            bare: false,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.width = size;
        self.height = size;
        self
    }


    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Tooltip text; defaults to the accessible label like the web's `IconButton`.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let color = if self.active {
            tokens.accent
        } else {
            self.color.unwrap_or(tokens.secondary)
        };
        let tooltip = self.tooltip.unwrap_or(self.label.clone());
        let mut button = div()
            .id(self.id)
            .w(sz(self.width))
            .h(sz(self.height))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(sz(6.0))
            .text_color(color)
            .child(self.child);
        if self.disabled {
            button = button.opacity(0.38);
        } else {
            let bare = self.bare;
            let hover_text = if self.active { tokens.accent } else if bare { gpui::white() } else { tokens.text };
            button = button
                .cursor_pointer()
                .hover(move |style| if bare { style.text_color(hover_text) } else { style.bg(tokens.hover).text_color(hover_text) })
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx));
            if let Some(handler) = self.on_click {
                button = button.on_click(handler);
            }
        }
        button
    }
}

/// Text helper: a single truncated line.
pub fn line(text: impl Into<SharedString>) -> gpui::Div {
    div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text.into())
}

