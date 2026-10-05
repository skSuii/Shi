//! Shi — CAD 主应用。
//! 菜单栏 + 绘图工具栏 + 图层面板 + 无限画布 + 右键上下文菜单 + 中英文界面。
//! 对齐 cadrs 内核：点/椭圆/样条/填充实体、移动/复制/旋转/缩放/镜像编辑、
//! 图层系统、对象捕捉（端点/中点/圆心/交点）、测量信息。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cadrs::data_structure::{
    clone_with_new_id, make_arc, make_circle, make_ellipse, make_hatch, make_line, make_point,
    make_polyline, make_spline, Document, Entity, EntityGeometry, Layer, ObjectId, SelectionSet,
};
use cadrs::dimension::strokes as dim;
use cadrs::edit::entity_transform::{mirror_entity, rotate_about, scale_about, transform_entity};
use cadrs::geometry::{Arc, Ellipse, Point};
use cadrs::history::op_history::{HistoryOp, OpHistory};
use cadrs::io::{dxf, eps, pdf, raster, svg, wmf};
use cadrs::math::Transform2D;
use cadrs::measurement::{entity_area, entity_length};
use cadrs::render::tessellation as tess;
use cadrs::snap::{intersection_candidates_capped, snap_candidates, SnapType as CadSnapType};
use egui::{
    pos2, Color32, Context, CornerRadius, Pos2, Rect, Response, RichText, Sense, Stroke, Ui, Vec2,
};

use crate::i18n::{self, Lang};
use crate::layers;

const TAU: f64 = std::f64::consts::TAU;

const BG: Color32 = Color32::from_rgb(28, 30, 34);
const LINE_COLOR: Color32 = Color32::from_rgb(212, 214, 220);
const SELECTED_COLOR: Color32 = Color32::from_rgb(255, 176, 32);
const GRID_MINOR: Color32 = Color32::from_rgb(45, 50, 58);
const GRID_MAJOR: Color32 = Color32::from_rgb(62, 68, 79);
const AXIS_X: Color32 = Color32::from_rgb(150, 78, 78);
const AXIS_Y: Color32 = Color32::from_rgb(78, 140, 90);
const ACCENT: Color32 = Color32::from_rgb(86, 156, 214);
const HINT_COLOR: Color32 = Color32::from_rgb(112, 118, 128);

const OSNAP_RADIUS: f32 = 14.0;

/// 新建图层循环使用的颜色
const LAYER_PALETTE: [(u8, u8, u8); 7] = [
    (212, 214, 220),
    (255, 96, 96),
    (96, 168, 255),
    (110, 220, 110),
    (255, 200, 90),
    (216, 128, 255),
    (255, 160, 200),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Line,
    Polyline,
    Circle,
    Arc,
    Rect,
    Point,
    Ellipse,
    Spline,
    Fill,
    DimLinear,
    DimRadial,
    DimDiameter,
    DimAngular,
    Move,
    Copy,
    Rotate,
    Scale,
    Mirror,
}

impl Tool {
    /// 是否为编辑变换工具（由菜单进入，不在工具栏）
    fn is_edit(self) -> bool {
        matches!(
            self,
            Tool::Move | Tool::Copy | Tool::Rotate | Tool::Scale | Tool::Mirror
        )
    }

    fn label(self, t: &i18n::Strings) -> &'static str {
        match self {
            Tool::Select => t.tool_select,
            Tool::Line => t.tool_line,
            Tool::Polyline => t.tool_polyline,
            Tool::Circle => t.tool_circle,
            Tool::Arc => t.tool_arc,
            Tool::Rect => t.tool_rect,
            Tool::Point => t.tool_point,
            Tool::Ellipse => t.tool_ellipse,
            Tool::Spline => t.tool_spline,
            Tool::Fill => t.tool_fill,
            Tool::DimLinear => t.tool_dim_linear,
            Tool::DimRadial => t.tool_dim_radial,
            Tool::DimDiameter => t.tool_dim_diameter,
            Tool::DimAngular => t.tool_dim_angular,
            Tool::Move => t.tool_move,
            Tool::Copy => t.tool_copy,
            Tool::Rotate => t.tool_rotate,
            Tool::Scale => t.tool_scale,
            Tool::Mirror => t.tool_mirror,
        }
    }

    fn hint(self, t: &i18n::Strings) -> &'static str {
        match self {
            Tool::Select => t.hint_select,
            Tool::Line => t.hint_line,
            Tool::Polyline => t.hint_polyline,
            Tool::Circle => t.hint_circle,
            Tool::Arc => t.hint_arc,
            Tool::Rect => t.hint_rect,
            Tool::Point => t.hint_point,
            Tool::Ellipse => t.hint_ellipse,
            Tool::Spline => t.hint_spline,
            Tool::Fill => t.hint_fill,
            Tool::DimLinear => t.hint_dim_linear,
            Tool::DimRadial => t.hint_dim_radial,
            Tool::DimDiameter => t.hint_dim_diameter,
            Tool::DimAngular => t.hint_dim_angular,
            Tool::Move => t.hint_move,
            Tool::Copy => t.hint_copy,
            Tool::Rotate => t.hint_rotate,
            Tool::Scale => t.hint_scale,
            Tool::Mirror => t.hint_mirror,
        }
    }
}

/// 对象捕捉类型
#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapKind {
    EndPoint,
    MidPoint,
    Center,
    Intersection,
}

/// 仅保留 Shi 支持的 4 种捕捉标记
fn snap_kind(kind: CadSnapType) -> Option<SnapKind> {
    match kind {
        CadSnapType::EndPoint => Some(SnapKind::EndPoint),
        CadSnapType::MidPoint => Some(SnapKind::MidPoint),
        CadSnapType::Center => Some(SnapKind::Center),
        CadSnapType::Intersection => Some(SnapKind::Intersection),
        _ => None,
    }
}

/// 逻辑加载格式
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Dxf,
    Svg,
    Json,
}

impl Format {
    fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            // DXF 家族统一按 ASCII DXF 解析（尽力而为）
            "dxf" | "dxx" | "dwg" | "dwt" | "dws" => Some(Format::Dxf),
            "svg" => Some(Format::Svg),
            "json" => Some(Format::Json),
            _ => None,
        }
    }
}

/// 导入对话框条目（决定过滤器扩展名）
#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportFormat {
    Dxf,
    Dxx,
    Dwg,
    Dwt,
    Dws,
    Svg,
    Json,
}

impl ImportFormat {
    fn ext(self) -> &'static str {
        match self {
            ImportFormat::Dxf => "dxf",
            ImportFormat::Dxx => "dxx",
            ImportFormat::Dwg => "dwg",
            ImportFormat::Dwt => "dwt",
            ImportFormat::Dws => "dws",
            ImportFormat::Svg => "svg",
            ImportFormat::Json => "json",
        }
    }

    fn logical(self) -> Format {
        match self {
            ImportFormat::Svg => Format::Svg,
            ImportFormat::Json => Format::Json,
            _ => Format::Dxf,
        }
    }

    fn filter_base<'a>(self, t: &'a i18n::Strings) -> &'a str {
        match self {
            ImportFormat::Dxf => t.f_dxf,
            ImportFormat::Dxx => t.f_dxx,
            ImportFormat::Dwg => t.f_dwg,
            ImportFormat::Dwt => t.f_dwt,
            ImportFormat::Dws => t.f_dws,
            ImportFormat::Svg => t.f_svg,
            ImportFormat::Json => t.f_json,
        }
    }
}

fn detect_format(path: &Path, content: &str) -> Format {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if let Some(f) = Format::from_extension(ext) {
            return f;
        }
    }
    let t = content.trim_start();
    if t.starts_with('<') {
        Format::Svg
    } else if t.starts_with('{') {
        Format::Json
    } else {
        Format::Dxf
    }
}

fn load_document(content: &str, fmt: Format) -> Result<Document, String> {
    match fmt {
        Format::Dxf => dxf::import(content),
        Format::Svg => svg::import(content),
        Format::Json => serde_json::from_str::<Document>(content).map_err(|e| e.to_string()),
    }
}

pub struct App {
    doc: Document,
    lang: Lang,
    tool: Tool,
    // 绘制草稿
    line_start: Option<Point>,
    poly_pts: Vec<Point>,
    circle_center: Option<Point>,
    arc_center: Option<Point>,
    arc_start: Option<Point>,
    rect_corner: Option<Point>,
    ellipse_center: Option<Point>,
    ellipse_major: Option<Point>,
    // 标注草稿：线性/角度标注拾取点；半径/直径标注目标 (圆心, 半径)
    dim_pts: Vec<Point>,
    radial_target: Option<(Point, f64)>,
    // 编辑变换拾取点
    edit_pts: Vec<Point>,
    // 框选起点（屏幕坐标）
    band_start: Option<Pos2>,
    hover_world: Option<Point>,
    active_snap: Option<(Point, SnapKind)>,
    selected: SelectionSet,
    history: OpHistory,
    pan: Vec2, // 画布左下角对应的世界坐标
    zoom: f64,
    grid_visible: bool,
    snap_enabled: bool,
    osnap_enabled: bool,
    grid_size: f64,
    // 图层
    current_layer: ObjectId,
    show_layers: bool,
    fill_color: [u8; 3],
    file_path: Option<PathBuf>,
    pending_fit: bool,
    pending_zoom: Option<f64>,
    status: String,
    show_about: bool,
    show_shortcuts: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        Self::install_cjk_fonts(&cc.egui_ctx);
        let lang = Lang::detect();
        let doc = Document::new(lang.strings().untitled.to_string());
        let current_layer = doc.model_space().clone();
        Self {
            doc,
            lang,
            tool: Tool::Line,
            line_start: None,
            poly_pts: Vec::new(),
            circle_center: None,
            arc_center: None,
            arc_start: None,
            rect_corner: None,
            ellipse_center: None,
            ellipse_major: None,
            dim_pts: Vec::new(),
            radial_target: None,
            edit_pts: Vec::new(),
            band_start: None,
            hover_world: None,
            active_snap: None,
            selected: SelectionSet::new(),
            history: OpHistory::new(),
            pan: Vec2::new(-100.0, -80.0),
            zoom: 2.0,
            grid_visible: true,
            snap_enabled: true,
            osnap_enabled: true,
            grid_size: 10.0,
            current_layer,
            show_layers: false,
            fill_color: [86, 156, 214],
            file_path: None,
            pending_fit: false,
            pending_zoom: None,
            status: lang.strings().s_welcome.to_string(),
            show_about: false,
            show_shortcuts: false,
        }
    }

    fn t(&self) -> &'static i18n::Strings {
        self.lang.strings()
    }

    /// egui 默认字体不含中文，从系统字体加载一个 CJK 字体作为回退。
    fn install_cjk_fonts(ctx: &Context) {
        let mut fonts = egui::FontDefinitions::default();
        let candidates = [
            "C:/Windows/Fonts/msyh.ttc",
            "C:/Windows/Fonts/Deng.ttf",
            "C:/Windows/Fonts/simhei.ttf",
        ];
        for path in candidates {
            if let Ok(data) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("cjk".into(), egui::FontData::from_owned(data).into());
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push("cjk".into());
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push("cjk".into());
                break;
            }
        }
        ctx.set_fonts(fonts);
    }

    fn apply_theme(ctx: &Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.selection.bg_fill = ACCENT;
        visuals.selection.stroke.color = Color32::WHITE;
        visuals.hyperlink_color = ACCENT;
        visuals.panel_fill = Color32::from_rgb(34, 36, 41);
        ctx.set_visuals(visuals);
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    // ---------- 图层辅助 ----------

    fn layer_info(&self, layer_id: &ObjectId) -> Option<layers::LayerInfo> {
        self.doc.get_layer(layer_id).map(layers::read)
    }

    fn layer_visible(&self, layer_id: &ObjectId) -> bool {
        self.layer_info(layer_id)
            .map(|i| i.visible)
            .unwrap_or(true)
    }

    /// 实体可交互：所在图层可见且未锁定
    fn entity_interactable(&self, entity: &Entity) -> bool {
        self.layer_info(&entity.layer_id)
            .map(|i| i.visible && !i.locked)
            .unwrap_or(true)
    }

    // ---------- 坐标变换 ----------

    fn world_to_screen(&self, p: Point, rect: Rect) -> Pos2 {
        Pos2::new(
            rect.left() + ((p.x - self.pan.x as f64) * self.zoom) as f32,
            rect.bottom() - ((p.y - self.pan.y as f64) * self.zoom) as f32,
        )
    }

    fn screen_to_world(&self, s: Pos2, rect: Rect) -> Point {
        Point::new2d(
            self.pan.x as f64 + (s.x - rect.left()) as f64 / self.zoom,
            self.pan.y as f64 + (rect.bottom() - s.y) as f64 / self.zoom,
        )
    }

    fn snap(&self, p: Point) -> Point {
        if !self.snap_enabled {
            return p;
        }
        let g = self.grid_size;
        Point::new2d((p.x / g).round() * g, (p.y / g).round() * g)
    }

    /// 点击用的世界坐标：对象捕捉优先，否则网格吸附
    fn snapped_world(&self, pos: Pos2, rect: Rect) -> Point {
        if let Some((p, _)) = self.active_snap {
            return p;
        }
        self.snap(self.screen_to_world(pos, rect))
    }

    /// 无限画布：缩放不作人为限制（仅保留浮点安全边界）
    fn zoom_at(&mut self, cursor: Pos2, rect: Rect, factor: f64) {
        let world = self.screen_to_world(cursor, rect);
        self.zoom = (self.zoom * factor).clamp(1e-6, 1e9);
        let pan_x = world.x - (cursor.x - rect.left()) as f64 / self.zoom;
        let pan_y = world.y - (rect.bottom() - cursor.y) as f64 / self.zoom;
        self.pan = Vec2::new(pan_x as f32, pan_y as f32);
    }

    fn fit_view(&mut self, rect: Rect) {
        let Some((min, max)) = tess::document_bbox(&self.doc) else {
            self.pan = Vec2::new(-100.0, -80.0);
            self.zoom = 2.0;
            return;
        };
        let w = (max.x - min.x).max(1e-3);
        let h = (max.y - min.y).max(1e-3);
        let zoom = ((rect.width() as f64 * 0.85) / w).min((rect.height() as f64 * 0.85) / h);
        self.zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        let cx = (min.x + max.x) / 2.0;
        let cy = (min.y + max.y) / 2.0;
        self.pan = Vec2::new(
            (cx - rect.width() as f64 / 2.0 / self.zoom) as f32,
            (cy - rect.height() as f64 / 2.0 / self.zoom) as f32,
        );
    }

    fn reset_view(&mut self) {
        self.pan = Vec2::new(-100.0, -80.0);
        self.zoom = 2.0;
        let t = self.t();
        self.set_status(t.s_view_reset);
    }

    // ---------- 草稿状态 ----------

    fn has_draft(&self) -> bool {
        self.line_start.is_some()
            || !self.poly_pts.is_empty()
            || self.circle_center.is_some()
            || self.arc_center.is_some()
            || self.arc_start.is_some()
            || self.rect_corner.is_some()
            || self.ellipse_center.is_some()
            || self.ellipse_major.is_some()
            || !self.dim_pts.is_empty()
            || self.radial_target.is_some()
            || !self.edit_pts.is_empty()
    }

    fn cancel_draft(&mut self) {
        self.line_start = None;
        self.poly_pts.clear();
        self.circle_center = None;
        self.arc_center = None;
        self.arc_start = None;
        self.rect_corner = None;
        self.ellipse_center = None;
        self.ellipse_major = None;
        self.dim_pts.clear();
        self.radial_target = None;
        self.edit_pts.clear();
        self.band_start = None;
    }

    /// 标注文字 / 箭头的世界尺寸（随网格步长）
    fn dim_text_height(&self) -> f64 {
        self.grid_size.max(1e-3)
    }

    fn finish_polyline(&mut self, closed: bool) {
        let t = self.t();
        if self.poly_pts.len() >= 2 {
            let pts = std::mem::take(&mut self.poly_pts);
            let entity = make_polyline(&pts, closed);
            let msg = if closed {
                t.s_add_polyline_closed
            } else {
                t.s_add_polyline
            };
            self.add_entity(entity, msg);
        } else {
            self.poly_pts.clear();
        }
    }

    fn finish_spline(&mut self) {
        let t = self.t();
        if self.poly_pts.len() >= 3 {
            let pts = std::mem::take(&mut self.poly_pts);
            if let Some(entity) = make_spline(&pts) {
                self.add_entity(entity, t.s_add_spline);
            }
        } else {
            self.poly_pts.clear();
        }
    }

    // ---------- 文档操作 ----------

    fn add_entity(&mut self, mut entity: Entity, msg: &str) {
        entity.layer_id = self.current_layer.clone();
        self.history.push(HistoryOp::Added(entity.clone()));
        self.doc.add_entity(entity);
        self.set_status(msg.to_string());
    }

    fn delete_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let t = self.t();
        let ids: Vec<ObjectId> = self.selected.entity_ids().iter().cloned().collect();
        let mut n = 0;
        for id in ids {
            if let Some(entity) = self.doc.get_entity(&id).cloned() {
                if self.doc.remove_entity(&id) {
                    self.history.push(HistoryOp::Removed(entity));
                    self.selected.remove(&id);
                    n += 1;
                }
            }
        }
        if n > 0 {
            self.set_status(t.s_deleted);
        }
    }

    /// 对选区应用编辑变换：tf 平移/旋转/缩放；mirror 镜像线。copy 保留原实体。
    fn edit_selection(
        &mut self,
        copy: bool,
        tf: Option<Transform2D>,
        mirror: Option<(Point, Point)>,
        msg: &str,
    ) {
        let ids: Vec<ObjectId> = self.selected.entity_ids().iter().cloned().collect();
        for id in ids {
            let Some(entity) = self.doc.get_entity(&id).cloned() else {
                continue;
            };
            let mut target = if copy {
                clone_with_new_id(&entity)
            } else {
                entity.clone()
            };
            if let Some(tf) = &tf {
                transform_entity(&mut target, tf);
            }
            if let Some((p1, p2)) = mirror {
                mirror_entity(&mut target, p1, p2);
            }
            if copy {
                self.doc.add_entity(target.clone());
                self.history.push(HistoryOp::Added(target));
            } else {
                self.history
                    .push(HistoryOp::Modified(entity, target.clone()));
                self.doc.entities_mut().insert(id, target);
            }
        }
        self.edit_pts.clear();
        self.set_status(msg.to_string());
    }

    fn clear_all(&mut self) {
        if self.doc.entity_count() == 0 {
            return;
        }
        let ids: Vec<ObjectId> = self.doc.entities().keys().cloned().collect();
        for id in ids {
            if let Some(entity) = self.doc.get_entity(&id).cloned() {
                if self.doc.remove_entity(&id) {
                    self.history.push(HistoryOp::Removed(entity));
                }
            }
        }
        self.selected.clear();
        self.cancel_draft();
        let t = self.t();
        self.set_status(t.s_cleared);
    }

    fn undo(&mut self) {
        let t = self.t();
        let before: HashSet<ObjectId> = self.doc.entities().keys().cloned().collect();
        if !self.history.undo(&mut self.doc) {
            self.set_status(t.s_no_undo);
            return;
        }
        let after: HashSet<ObjectId> = self.doc.entities().keys().cloned().collect();
        let mut restored = false;
        for id in &before {
            if !after.contains(id) {
                self.selected.remove(id);
            }
        }
        for id in &after {
            if !before.contains(id) {
                self.selected.add(id.clone());
                restored = true;
            }
        }
        if restored {
            self.set_status(t.s_restored);
        } else {
            self.set_status(t.s_undone);
        }
    }

    fn redo(&mut self) {
        let t = self.t();
        let before: HashSet<ObjectId> = self.doc.entities().keys().cloned().collect();
        if !self.history.redo(&mut self.doc) {
            self.set_status(t.s_no_redo);
            return;
        }
        let after: HashSet<ObjectId> = self.doc.entities().keys().cloned().collect();
        for id in &before {
            if !after.contains(id) {
                self.selected.remove(id);
            }
        }
        self.set_status(t.s_redone);
    }

    fn new_document(&mut self) {
        let name = self.t().untitled.to_string();
        self.doc = Document::new(name);
        self.history.clear();
        self.selected.clear();
        self.cancel_draft();
        self.current_layer = self.doc.model_space().clone();
        self.file_path = None;
        self.pending_fit = true;
        let t = self.t();
        self.set_status(t.s_new_doc);
    }

    fn with_extension(path: PathBuf, ext: &str) -> PathBuf {
        if path.extension().is_none() {
            path.with_extension(ext)
        } else {
            path
        }
    }

    fn open_path(&mut self, path: PathBuf, fmt_hint: Option<Format>) {
        let t = self.t();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                self.set_status(t.s_read_fail.replace("{}", &e.to_string()));
                return;
            }
        };
        let fmt = fmt_hint.unwrap_or_else(|| detect_format(&path, &content));
        match load_document(&content, fmt) {
            Ok(doc) => {
                self.doc = doc;
                self.history.clear();
                self.selected.clear();
                self.cancel_draft();
                self.current_layer = self.doc.model_space().clone();
                self.file_path = if fmt == Format::Dxf {
                    Some(path.clone())
                } else {
                    None
                };
                self.pending_fit = true;
                self.set_status(t.s_opened.replace("{}", &path.display().to_string()));
            }
            Err(e) => self.set_status(t.s_import_fail.replace("{}", &e)),
        }
    }

    fn do_open(&mut self) {
        let t = self.t();
        let path = rfd::FileDialog::new()
            .add_filter(t.f_cad_all, &["dxf", "dxx", "dwg", "dwt", "dws"])
            .add_filter(t.f_svg, &["svg"])
            .add_filter(t.f_json, &["json"])
            .pick_file();
        if let Some(path) = path {
            self.open_path(path, None);
        }
    }

    fn do_import(&mut self, fmt: ImportFormat) {
        let t = self.t();
        let ext = fmt.ext();
        let filter = format!("{} (*.{ext})", fmt.filter_base(t));
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(&filter, &[ext])
            .pick_file()
        {
            self.open_path(path, Some(fmt.logical()));
        }
    }

    fn do_save(&mut self) {
        let t = self.t();
        let Some(path) = self.file_path.clone() else {
            self.do_save_as();
            return;
        };
        let content = dxf::export(&self.doc);
        match std::fs::write(&path, content) {
            Ok(()) => self.set_status(t.s_saved.replace("{}", &path.display().to_string())),
            Err(e) => self.set_status(t.s_save_fail.replace("{}", &e.to_string())),
        }
    }

    fn do_save_as(&mut self) {
        let t = self.t();
        let filter = format!("{} (*.dxf)", t.f_dxf);
        let path = rfd::FileDialog::new()
            .add_filter(&filter, &["dxf"])
            .set_file_name(format!("{}.dxf", t.untitled))
            .save_file();
        if let Some(path) = path {
            self.file_path = Some(Self::with_extension(path, "dxf"));
            self.do_save();
        }
    }

    fn save_bytes(&mut self, path: &Path, bytes: &[u8]) {
        let t = self.t();
        match std::fs::write(path, bytes) {
            Ok(()) => self.set_status(t.s_exported.replace("{}", &path.display().to_string())),
            Err(e) => self.set_status(t.s_export_fail.replace("{}", &e.to_string())),
        }
    }

    /// 通用导出：对话框 + 生成字节 + 写盘
    fn save_export(
        &mut self,
        ext: &str,
        filter: &str,
        make: impl FnOnce(&Document) -> Result<Vec<u8>, String>,
    ) {
        let t = self.t();
        if self.doc.entity_count() == 0 {
            self.set_status(t.s_empty_canvas);
            return;
        }
        let path = rfd::FileDialog::new()
            .add_filter(filter, &[ext])
            .set_file_name(format!("drawing.{ext}"))
            .save_file();
        let Some(path) = path else {
            return;
        };
        let path = Self::with_extension(path, ext);
        match make(&self.doc) {
            Ok(bytes) => self.save_bytes(&path, &bytes),
            Err(e) => self.set_status(t.s_export_fail.replace("{}", &e)),
        }
    }

    fn do_export_dxf(&mut self) {
        let t = self.t();
        let filter = format!("{} (*.dxf)", t.f_dxf);
        self.save_export("dxf", &filter, |doc| Ok(dxf::export(doc).into_bytes()));
    }

    fn do_export_svg(&mut self) {
        let t = self.t();
        let filter = format!("{} (*.svg)", t.f_svg);
        self.save_export("svg", &filter, |doc| {
            svg::export(doc).map(|s| s.into_bytes())
        });
    }

    fn do_export_eps(&mut self) {
        let t = self.t();
        let filter = format!("{} (*.eps)", t.f_eps);
        self.save_export("eps", &filter, |doc| {
            eps::export(doc).map(|s| s.into_bytes())
        });
    }

    fn do_export_pdf(&mut self) {
        let t = self.t();
        let filter = format!("{} (*.pdf)", t.f_pdf);
        self.save_export("pdf", &filter, pdf::export);
    }

    fn do_export_wmf(&mut self) {
        let t = self.t();
        let filter = format!("{} (*.wmf)", t.f_wmf);
        self.save_export("wmf", &filter, wmf::export);
    }

    fn do_export_raster(&mut self, fmt: raster::RasterFormat) {
        let t = self.t();
        let base = match fmt {
            raster::RasterFormat::Png => t.f_png,
            raster::RasterFormat::Bmp => t.f_bmp,
            raster::RasterFormat::Jpeg => t.f_jpeg,
            raster::RasterFormat::WebP => t.f_webp,
        };
        let ext = fmt.extension();
        let filter = format!("{} (*.{ext})", base);
        self.save_export(ext, &filter, |doc| raster::export(doc, fmt));
    }

    // ---------- 快捷键 ----------

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let (undo, redo, save, new, open, escape, delete, enter, fit, al, ar, au, ad) =
            ctx.input(|i| {
                (
                    i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Y),
                    i.modifiers.ctrl && i.key_pressed(egui::Key::S),
                    i.modifiers.ctrl && i.key_pressed(egui::Key::N),
                    i.modifiers.ctrl && i.key_pressed(egui::Key::O),
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Delete),
                    i.key_pressed(egui::Key::Enter),
                    !i.modifiers.ctrl
                        && !i.modifiers.shift
                        && !i.modifiers.alt
                        && i.key_pressed(egui::Key::F),
                    i.key_pressed(egui::Key::ArrowLeft),
                    i.key_pressed(egui::Key::ArrowRight),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                )
            });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
        if save {
            self.do_save();
        }
        if new {
            self.new_document();
        }
        if open {
            self.do_open();
        }
        if escape {
            self.cancel_draft();
            self.selected.clear();
        }
        if delete {
            self.delete_selected();
        }
        if enter && self.tool == Tool::Polyline {
            self.finish_polyline(false);
        }
        if enter && self.tool == Tool::Spline {
            self.finish_spline();
        }
        if fit {
            self.pending_fit = true;
        }
        let step = (60.0 / self.zoom) as f32;
        if al {
            self.pan.x -= step;
        }
        if ar {
            self.pan.x += step;
        }
        if au {
            self.pan.y += step;
        }
        if ad {
            self.pan.y -= step;
        }
    }

    // ---------- 对象捕捉 ----------

    /// 计算光标附近的对象捕捉点：端点 / 中点 / 圆心 / 交点
    fn compute_osnap(&self, cursor: Pos2, rect: Rect) -> Option<(Point, SnapKind)> {
        let mut best: Option<(f32, Point, SnapKind)> = None;
        {
            let mut consider = |p: Point, kind: SnapKind| {
                let s = self.world_to_screen(p, rect);
                let d = s.distance(cursor);
                if d <= OSNAP_RADIUS && best.map_or(true, |(bd, _, _)| d < bd) {
                    best = Some((d, p, kind));
                }
            };
            let mut near: Vec<&Entity> = Vec::new();
            for entity in self.doc.entities().values() {
                if !self.layer_visible(&entity.layer_id) {
                    continue;
                }
                // 捕捉候选（端点 / 中点 / 圆心等；仅取 Shi 支持的 4 种标记）
                for (p, kind) in snap_candidates(entity) {
                    if let Some(kind) = snap_kind(kind) {
                        consider(p, kind);
                    }
                }
                // 附近实体参与交点计算
                let mut any_near = false;
                for (pts, _) in tess::entity_polylines(entity) {
                    for p in pts.iter().step_by(4) {
                        if self.world_to_screen(*p, rect).distance(cursor) < OSNAP_RADIUS * 3.0 {
                            any_near = true;
                            break;
                        }
                    }
                    if any_near {
                        break;
                    }
                }
                if any_near && near.len() < 6 {
                    near.push(entity);
                }
            }
            // 交点（实体两两求交，仅限光标附近实体）
            for i in 0..near.len() {
                for j in i + 1..near.len() {
                    for ip in intersection_candidates_capped(near[i], near[j], 32) {
                        consider(ip, SnapKind::Intersection);
                    }
                }
            }
        }
        best.map(|(_, p, k)| (p, k))
    }

    // ---------- 画布交互 ----------

    fn pick_entity(&self, cursor: Pos2, rect: Rect) -> Option<ObjectId> {
        let mut best: Option<(f32, ObjectId)> = None;
        for (id, entity) in self.doc.entities() {
            if !self.entity_interactable(entity) {
                continue;
            }
            let mut min_d = f32::INFINITY;
            // 点实体：屏幕距离
            if let EntityGeometry::Point(p) = entity.geometry() {
                min_d = cursor.distance(self.world_to_screen(*p, rect));
            } else {
                for (pts, closed) in tess::entity_polylines(entity) {
                    let n = pts.len();
                    if n < 2 {
                        continue;
                    }
                    let scr: Vec<Pos2> =
                        pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                    for i in 0..n - 1 {
                        min_d = min_d.min(dist_point_to_segment(cursor, scr[i], scr[i + 1]));
                    }
                    if closed {
                        min_d = min_d.min(dist_point_to_segment(cursor, scr[n - 1], scr[0]));
                    }
                }
            }
            if min_d < 8.0 && best.as_ref().map_or(true, |(bd, _)| min_d < *bd) {
                best = Some((min_d, id.clone()));
            }
        }
        best.map(|(_, id)| id)
    }

    /// 框选：实体任一顶点落在矩形内或任一线段与矩形相交即选中
    fn pick_entities_rect(&self, band: Rect, rect: Rect) -> Vec<ObjectId> {
        let mut hits = Vec::new();
        for (id, entity) in self.doc.entities() {
            if !self.entity_interactable(entity) {
                continue;
            }
            let mut hit = false;
            if let EntityGeometry::Point(p) = entity.geometry() {
                hit = band.contains(self.world_to_screen(*p, rect));
            }
            if !hit {
                'outer: for (pts, _) in tess::entity_polylines(entity) {
                    let scr: Vec<Pos2> = pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                    for s in &scr {
                        if band.contains(*s) {
                            hit = true;
                            break 'outer;
                        }
                    }
                    for w in scr.windows(2) {
                        if seg_hits_rect(w[0], w[1], band) {
                            hit = true;
                            break 'outer;
                        }
                    }
                }
            }
            if hit {
                hits.push(id.clone());
            }
        }
        hits
    }

    fn handle_canvas_input(&mut self, response: &Response, rect: Rect) {
        // 注意：egui 0.36 的 interact_pointer_pos 仅在按钮按下/点击瞬间有值，
        // 纯悬停必须用 hover_pos，否则草稿预览（橡皮筋）不会跟随光标。
        let pointer_pos = response
            .hover_pos()
            .or_else(|| response.interact_pointer_pos());
        self.hover_world = pointer_pos.map(|p| self.screen_to_world(p, rect));

        // 对象捕捉（绘图/编辑工具）
        self.active_snap = None;
        if self.osnap_enabled && self.tool != Tool::Select {
            if let Some(pos) = pointer_pos {
                self.active_snap = self.compute_osnap(pos, rect);
            }
        }

        // 平移：中键始终平移；左键拖动在非 Select 工具下平移（Select 下左键拖动为框选）
        let drag = response.drag_delta();
        if response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Primary) && self.tool != Tool::Select)
        {
            self.pan.x -= drag.x / self.zoom as f32;
            self.pan.y += drag.y / self.zoom as f32;
        }

        // 框选（Select 工具）
        if self.tool == Tool::Select {
            if response.drag_started_by(egui::PointerButton::Primary) {
                self.band_start = response.interact_pointer_pos();
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                if let Some(start) = self.band_start.take() {
                    if let Some(end) = response.interact_pointer_pos() {
                        let band = Rect::from_two_pos(start, end);
                        if band.width() > 4.0 || band.height() > 4.0 {
                            let hits = self.pick_entities_rect(band, rect);
                            let shift = response.ctx.input(|i| i.modifiers.shift);
                            if !shift {
                                self.selected.clear();
                            }
                            let n = hits.len();
                            self.selected.add_multiple(&hits);
                            let t = self.t();
                            self.set_status(format!("{} {}", t.sb_selected, n));
                        }
                    }
                }
            }
        } else {
            self.band_start = None;
        }

        // 缩放：滚轮，以光标为中心
        let scroll: f32 = response.ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                    _ => None,
                })
                .sum()
        });
        if response.hovered() && scroll.abs() > 0.0 {
            if let Some(pos) = response.hover_pos() {
                let factor = 1.1f32.powf(scroll.clamp(-10.0, 10.0)) as f64;
                self.zoom_at(pos, rect, factor);
            }
        }

        let Some(pos) = response.interact_pointer_pos() else {
            return;
        };
        let t = self.t();
        match self.tool {
            Tool::Select => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let shift = response.ctx.input(|i| i.modifiers.shift);
                    match self.pick_entity(pos, rect) {
                        Some(id) => {
                            if shift {
                                if self.selected.contains(&id) {
                                    self.selected.remove(&id);
                                } else {
                                    self.selected.add(id);
                                }
                            } else {
                                self.selected.clear();
                                self.selected.add(id);
                            }
                        }
                        None => {
                            if !shift {
                                self.selected.clear();
                            }
                        }
                    }
                }
            }
            Tool::Line => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    match self.line_start.take() {
                        None => self.line_start = Some(world),
                        Some(start) => {
                            if start.distance_to(&world) > 1e-9 {
                                let entity = make_line(start, world);
                                self.add_entity(entity, t.s_add_line);
                            }
                        }
                    }
                }
            }
            Tool::Polyline => {
                if response.double_clicked_by(egui::PointerButton::Primary) {
                    self.finish_polyline(false);
                } else if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    if self.poly_pts.len() >= 3
                        && self.poly_pts[0].distance_to(&world) <= 1e-9
                    {
                        // 单击回到起点 → 闭合完成
                        self.finish_polyline(true);
                    } else if self
                        .poly_pts
                        .last()
                        .map_or(true, |l| l.distance_to(&world) > 1e-9)
                    {
                        self.poly_pts.push(world);
                        if self.poly_pts.len() == 1 {
                            self.set_status(t.s_poly_continue);
                        }
                    }
                }
            }
            Tool::Spline => {
                if response.double_clicked_by(egui::PointerButton::Primary) {
                    self.finish_spline();
                } else if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    if self
                        .poly_pts
                        .last()
                        .map_or(true, |l| l.distance_to(&world) > 1e-9)
                    {
                        self.poly_pts.push(world);
                    }
                }
            }
            Tool::Circle => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    match self.circle_center.take() {
                        None => self.circle_center = Some(world),
                        Some(center) => {
                            let r = center.distance_to(&world);
                            if r > 1e-9 {
                                let entity = make_circle(center, r);
                                self.add_entity(entity, t.s_add_circle);
                            }
                        }
                    }
                }
            }
            Tool::Arc => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    match (self.arc_center, self.arc_start) {
                        (None, _) => self.arc_center = Some(world),
                        (Some(c), None) => {
                            if c.distance_to(&world) > 1e-9 {
                                self.arc_start = Some(world);
                            }
                        }
                        (Some(c), Some(s)) => {
                            self.arc_center = None;
                            self.arc_start = None;
                            let r = c.distance_to(&s);
                            let a0 = (s.y - c.y).atan2(s.x - c.x);
                            let a1 = (world.y - c.y).atan2(world.x - c.x);
                            let span = ((a1 - a0) % TAU + TAU) % TAU;
                            if r > 1e-9 && span > 1e-9 {
                                let entity = make_arc(c, r, a0, a1);
                                self.add_entity(entity, t.s_add_arc);
                            }
                        }
                    }
                }
            }
            Tool::Rect => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    match self.rect_corner.take() {
                        None => self.rect_corner = Some(world),
                        Some(c) => {
                            if (world.x - c.x).abs() > 1e-9 && (world.y - c.y).abs() > 1e-9 {
                                let pts = [
                                    Point::new2d(c.x, c.y),
                                    Point::new2d(world.x, c.y),
                                    Point::new2d(world.x, world.y),
                                    Point::new2d(c.x, world.y),
                                ];
                                let entity = make_polyline(&pts, true);
                                self.add_entity(entity, t.s_add_rect);
                            }
                        }
                    }
                }
            }
            Tool::Point => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    let entity = make_point(world);
                    self.add_entity(entity, t.s_add_point);
                }
            }
            Tool::Ellipse => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    match (self.ellipse_center, self.ellipse_major) {
                        (None, _) => self.ellipse_center = Some(world),
                        (Some(c), None) => {
                            if c.distance_to(&world) > 1e-9 {
                                self.ellipse_major = Some(world);
                            }
                        }
                        (Some(c), Some(m)) => {
                            self.ellipse_center = None;
                            self.ellipse_major = None;
                            let rot = (m.y - c.y).atan2(m.x - c.x);
                            let a = c.distance_to(&m);
                            // 短轴 = 光标到长轴线的垂直距离
                            let dx = m.x - c.x;
                            let dy = m.y - c.y;
                            let len_sq = dx * dx + dy * dy;
                            let tproj = if len_sq > 1e-12 {
                                ((world.x - c.x) * dx + (world.y - c.y) * dy) / len_sq
                            } else {
                                0.0
                            };
                            let proj = Point::new2d(c.x + tproj * dx, c.y + tproj * dy);
                            let b = world.distance_to(&proj);
                            if a > 1e-9 && b > 1e-9 {
                                let entity = make_ellipse(c, a, b, rot);
                                self.add_entity(entity, t.s_add_ellipse);
                            }
                        }
                    }
                }
            }
            Tool::Fill => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let Some(id) = self.pick_entity(pos, rect) else {
                        self.set_status(t.s_need_closed);
                        return;
                    };
                    let Some(target) = self.doc.get_entity(&id) else {
                        return;
                    };
                    let boundary: Option<Vec<Point>> = match target.geometry() {
                        EntityGeometry::Circle(c) => {
                            Some(tess::circle_points(c.center, c.radius, 72))
                        }
                        EntityGeometry::Ellipse(e) => Some(tess::ellipse_points(e, 72)),
                        EntityGeometry::Polyline(p)
                            if p.is_closed && p.vertices.len() >= 3 =>
                        {
                            Some(
                                p.vertices
                                    .iter()
                                    .map(|v| Point::new2d(v.x, v.y))
                                    .collect(),
                            )
                        }
                        _ => None,
                    };
                    let color = (self.fill_color[0], self.fill_color[1], self.fill_color[2]);
                    match boundary.and_then(|b| make_hatch(&b, color)) {
                        Some(entity) => self.add_entity(entity, t.s_add_fill),
                        None => self.set_status(t.s_need_closed),
                    }
                }
            }
            Tool::DimLinear => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    match self.dim_pts.len() {
                        0 => self.dim_pts.push(world),
                        1 => {
                            if self.dim_pts[0].distance_to(&world) > 1e-9 {
                                self.dim_pts.push(world);
                            }
                        }
                        _ => {
                            let (p1, p2) = (self.dim_pts[0], self.dim_pts[1]);
                            self.dim_pts.clear();
                            if let Some(entity) =
                                dim::make_linear(p1, p2, world, self.dim_text_height())
                            {
                                self.add_entity(entity, t.s_add_dim);
                            }
                        }
                    }
                }
            }
            Tool::DimRadial | Tool::DimDiameter => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    match self.radial_target {
                        Some((center, radius)) => {
                            self.radial_target = None;
                            let aim = self.snapped_world(pos, rect);
                            if let Some(entity) = dim::make_radial(
                                center,
                                radius,
                                aim,
                                self.tool == Tool::DimDiameter,
                                self.dim_text_height(),
                            ) {
                                self.add_entity(entity, t.s_add_dim);
                            }
                        }
                        None => {
                            // 在光标附近找圆 / 圆弧
                            let target = self
                                .pick_entity(pos, rect)
                                .and_then(|id| self.doc.get_entity(&id))
                                .and_then(|e| match e.geometry() {
                                    EntityGeometry::Circle(c) => Some((c.center, c.radius)),
                                    EntityGeometry::Arc(a) => Some((a.center, a.radius)),
                                    _ => None,
                                });
                            match target {
                                Some(tr) => self.radial_target = Some(tr),
                                None => self.set_status(t.s_need_circle),
                            }
                        }
                    }
                }
            }
            Tool::DimAngular => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    let world = self.snapped_world(pos, rect);
                    match self.dim_pts.len() {
                        0 => self.dim_pts.push(world),
                        1 | 2 => {
                            if self.dim_pts[self.dim_pts.len() - 1].distance_to(&world) > 1e-9 {
                                self.dim_pts.push(world);
                            }
                            if self.dim_pts.len() == 3 {
                                let (v, r1, r2) = (self.dim_pts[0], self.dim_pts[1], self.dim_pts[2]);
                                self.dim_pts.clear();
                                if let Some(entity) =
                                    dim::make_angular(v, r1, r2, self.dim_text_height())
                                {
                                    self.add_entity(entity, t.s_add_dim);
                                }
                            }
                        }
                        _ => self.dim_pts.clear(),
                    }
                }
            }
            Tool::Move | Tool::Copy => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    if self.selected.is_empty() {
                        self.set_status(t.s_none_selected);
                        return;
                    }
                    let world = self.snapped_world(pos, rect);
                    self.edit_pts.push(world);
                    if self.edit_pts.len() == 2 {
                        let (from, to) = (self.edit_pts[0], self.edit_pts[1]);
                        let copy = self.tool == Tool::Copy;
                        let tf = Transform2D::from_translation(to.x - from.x, to.y - from.y);
                        let msg = if copy { t.s_edit_copied } else { t.s_edit_moved };
                        self.edit_selection(copy, Some(tf), None, msg);
                    }
                }
            }
            Tool::Rotate => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    if self.selected.is_empty() {
                        self.set_status(t.s_none_selected);
                        return;
                    }
                    let world = self.snapped_world(pos, rect);
                    self.edit_pts.push(world);
                    if self.edit_pts.len() == 3 {
                        let (c, r, target) = (self.edit_pts[0], self.edit_pts[1], self.edit_pts[2]);
                        let a0 = (r.y - c.y).atan2(r.x - c.x);
                        let a1 = (target.y - c.y).atan2(target.x - c.x);
                        let tf = rotate_about(c, a1 - a0);
                        self.edit_selection(false, Some(tf), None, t.s_edit_rotated);
                    }
                }
            }
            Tool::Scale => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    if self.selected.is_empty() {
                        self.set_status(t.s_none_selected);
                        return;
                    }
                    let world = self.snapped_world(pos, rect);
                    self.edit_pts.push(world);
                    if self.edit_pts.len() == 3 {
                        let (c, r, target) = (self.edit_pts[0], self.edit_pts[1], self.edit_pts[2]);
                        let d0 = c.distance_to(&r);
                        let d1 = c.distance_to(&target);
                        if d0 > 1e-9 && d1 > 1e-9 {
                            let tf = scale_about(c, d1 / d0);
                            self.edit_selection(false, Some(tf), None, t.s_edit_scaled);
                        } else {
                            self.edit_pts.clear();
                        }
                    }
                }
            }
            Tool::Mirror => {
                if response.clicked_by(egui::PointerButton::Primary) {
                    if self.selected.is_empty() {
                        self.set_status(t.s_none_selected);
                        return;
                    }
                    let world = self.snapped_world(pos, rect);
                    self.edit_pts.push(world);
                    if self.edit_pts.len() == 2 {
                        let (p1, p2) = (self.edit_pts[0], self.edit_pts[1]);
                        self.edit_selection(false, None, Some((p1, p2)), t.s_edit_mirrored);
                    }
                }
            }
        }
    }

    // ---------- 绘制 ----------

    fn paint_grid(&self, painter: &egui::Painter, rect: Rect) {
        let mut step = self.grid_size;
        // 缩小时合并网格线；无限画布下限制步长上限防止死循环
        while step * self.zoom < 8.0 && step < 1e12 {
            step *= 5.0;
        }
        // 放大时细分网格线
        while step * self.zoom > 60.0 && step > self.grid_size * 1e-6 {
            step /= 5.0;
        }
        let left = self.pan.x as f64;
        let right = left + rect.width() as f64 / self.zoom;
        let bottom = self.pan.y as f64;
        let top = bottom + rect.height() as f64 / self.zoom;

        let mut x = (left / step).floor() * step;
        while x <= right {
            let sx = rect.left() + ((x - left) * self.zoom) as f32;
            let major = (x / step).round() as i64 % 5 == 0;
            let color = if major { GRID_MAJOR } else { GRID_MINOR };
            painter.line_segment(
                [pos2(sx, rect.top()), pos2(sx, rect.bottom())],
                Stroke::new(1.0, color),
            );
            x += step;
        }
        let mut y = (bottom / step).floor() * step;
        while y <= top {
            let sy = rect.bottom() - ((y - bottom) * self.zoom) as f32;
            let major = (y / step).round() as i64 % 5 == 0;
            let color = if major { GRID_MAJOR } else { GRID_MINOR };
            painter.line_segment(
                [pos2(rect.left(), sy), pos2(rect.right(), sy)],
                Stroke::new(1.0, color),
            );
            y += step;
        }
    }

    /// 以强调色绘制实体分解笔画（标注草稿预览用）
    fn paint_entity_preview(&self, painter: &egui::Painter, entity: &Entity, rect: Rect) {
        if let EntityGeometry::Point(p) = entity.geometry() {
            paint_marker(painter, self.world_to_screen(*p, rect), ACCENT, 5.0);
            return;
        }
        for (pts, closed) in tess::entity_polylines(entity) {
            if pts.len() < 2 {
                continue;
            }
            let scr: Vec<Pos2> = pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
            paint_polyline(painter, &scr, closed, Stroke::new(1.5, ACCENT));
        }
    }

    /// 编辑变换预览：克隆选区并应用变换后绘制
    fn paint_edit_preview(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        tf: Option<Transform2D>,
        mirror: Option<(Point, Point)>,
    ) {
        for id in self.selected.entity_ids() {
            let Some(entity) = self.doc.get_entity(id) else {
                continue;
            };
            let mut e = entity.clone();
            if let Some(t) = &tf {
                transform_entity(&mut e, t);
            }
            if let Some((p1, p2)) = mirror {
                mirror_entity(&mut e, p1, p2);
            }
            for (pts, color) in tess::entity_fills(&e) {
                if pts.len() < 3 {
                    continue;
                }
                let scr: Vec<Pos2> = pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                painter.add(egui::Shape::convex_polygon(
                    scr,
                    Color32::from_rgba_unmultiplied(color.0, color.1, color.2, 90),
                    Stroke::NONE,
                ));
            }
            self.paint_entity_preview(painter, &e, rect);
        }
    }

    fn paint_canvas(&self, painter: &egui::Painter, rect: Rect) {
        painter.rect_filled(rect, CornerRadius::ZERO, BG);

        if self.grid_visible {
            self.paint_grid(painter, rect);
        }

        // 世界坐标轴
        let origin = self.world_to_screen(Point::origin(), rect);
        if origin.y >= rect.top() && origin.y <= rect.bottom() {
            painter.line_segment(
                [pos2(rect.left(), origin.y), pos2(rect.right(), origin.y)],
                Stroke::new(1.5, AXIS_X),
            );
        }
        if origin.x >= rect.left() && origin.x <= rect.right() {
            painter.line_segment(
                [pos2(origin.x, rect.top()), pos2(origin.x, rect.bottom())],
                Stroke::new(1.5, AXIS_Y),
            );
        }

        // 填充（先画填充，再画笔画）
        for (id, entity) in self.doc.entities() {
            let _ = id;
            if !self.layer_visible(&entity.layer_id) {
                continue;
            }
            for (pts, color) in tess::entity_fills(entity) {
                if pts.len() < 3 {
                    continue;
                }
                let scr: Vec<Pos2> = pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                painter.add(egui::Shape::convex_polygon(
                    scr,
                    Color32::from_rgb(color.0, color.1, color.2),
                    Stroke::NONE,
                ));
            }
        }

        // 实体
        for (id, entity) in self.doc.entities() {
            if !self.layer_visible(&entity.layer_id) {
                continue;
            }
            let is_selected = self.selected.contains(id);
            let color = if is_selected {
                SELECTED_COLOR
            } else {
                self.layer_info(&entity.layer_id)
                    .map(|i| Color32::from_rgb(i.color.0, i.color.1, i.color.2))
                    .unwrap_or(LINE_COLOR)
            };
            let width = if is_selected { 2.5 } else { 1.5 };
            // 点实体：小十字
            if let EntityGeometry::Point(p) = entity.geometry() {
                let s = self.world_to_screen(*p, rect);
                painter.line_segment(
                    [pos2(s.x - 4.0, s.y), pos2(s.x + 4.0, s.y)],
                    Stroke::new(width, color),
                );
                painter.line_segment(
                    [pos2(s.x, s.y - 4.0), pos2(s.x, s.y + 4.0)],
                    Stroke::new(width, color),
                );
            }
            let strokes = tess::entity_polylines(entity);
            for (pts, closed) in &strokes {
                if pts.len() < 2 {
                    continue;
                }
                let scr: Vec<Pos2> = pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                paint_polyline(painter, &scr, *closed, Stroke::new(width, color));
            }
            if is_selected {
                let key: Vec<Pos2> = match entity.geometry() {
                    EntityGeometry::Point(p) => vec![self.world_to_screen(*p, rect)],
                    EntityGeometry::Line(l) => vec![
                        self.world_to_screen(l.start, rect),
                        self.world_to_screen(l.end, rect),
                    ],
                    EntityGeometry::Circle(c) => vec![self.world_to_screen(c.center, rect)],
                    EntityGeometry::Arc(a) => vec![
                        self.world_to_screen(a.center, rect),
                        self.world_to_screen(a.start_point(), rect),
                        self.world_to_screen(a.end_point(), rect),
                    ],
                    EntityGeometry::Polyline(p) => p
                        .vertices
                        .iter()
                        .map(|v| self.world_to_screen(Point::new2d(v.x, v.y), rect))
                        .collect(),
                    EntityGeometry::Dimension {
                        def_point_1,
                        def_point_2,
                        text_position,
                        ..
                    } => vec![
                        self.world_to_screen(*def_point_1, rect),
                        self.world_to_screen(*def_point_2, rect),
                        self.world_to_screen(*text_position, rect),
                    ],
                    EntityGeometry::Text { position, .. } => {
                        vec![self.world_to_screen(*position, rect)]
                    }
                    _ => Vec::new(),
                };
                for p in key {
                    paint_marker(painter, p, SELECTED_COLOR, 7.0);
                }
            }
        }

        let snapped_hover = self.hover_world.map(|h| {
            if let Some((p, _)) = self.active_snap {
                p
            } else {
                self.snap(h)
            }
        });

        // 画线草稿
        if let Some(start) = self.line_start {
            let ps = self.world_to_screen(start, rect);
            if let Some(h) = snapped_hover {
                painter.line_segment(
                    [ps, self.world_to_screen(h, rect)],
                    Stroke::new(1.5, ACCENT),
                );
            }
            paint_marker(painter, ps, ACCENT, 6.0);
        }

        // 多段线 / 样条草稿
        if !self.poly_pts.is_empty() {
            let scr: Vec<Pos2> = self
                .poly_pts
                .iter()
                .map(|p| self.world_to_screen(*p, rect))
                .collect();
            let mut all = scr.clone();
            if let Some(h) = snapped_hover {
                all.push(self.world_to_screen(h, rect));
            }
            paint_polyline(painter, &all, false, Stroke::new(1.5, ACCENT));
            for p in scr {
                paint_marker(painter, p, ACCENT, 5.0);
            }
            // 样条曲线预览
            if self.tool == Tool::Spline && self.poly_pts.len() >= 2 {
                if let Some(h) = snapped_hover {
                    let mut pts = self.poly_pts.clone();
                    pts.push(h);
                    if let Some(entity) = make_spline(&pts) {
                        self.paint_entity_preview(painter, &entity, rect);
                    }
                }
            }
        }

        // 圆草稿
        if let Some(center) = self.circle_center {
            let cs = self.world_to_screen(center, rect);
            if let Some(h) = snapped_hover {
                let hs = self.world_to_screen(h, rect);
                let r = center.distance_to(&h);
                if r > 1e-9 {
                    painter.line_segment([cs, hs], Stroke::new(1.0, HINT_COLOR));
                    let pts = tess::circle_points(center, r, 72);
                    let scr: Vec<Pos2> =
                        pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                    paint_polyline(painter, &scr, true, Stroke::new(1.5, ACCENT));
                }
            }
            paint_marker(painter, cs, ACCENT, 6.0);
        }

        // 圆弧草稿
        if let Some(center) = self.arc_center {
            let cs = self.world_to_screen(center, rect);
            paint_marker(painter, cs, ACCENT, 6.0);
            if let Some(start) = self.arc_start {
                let ss = self.world_to_screen(start, rect);
                paint_marker(painter, ss, ACCENT, 6.0);
                let r = center.distance_to(&start);
                if r > 1e-9 {
                    if let Some(h) = snapped_hover {
                        let a0 = (start.y - center.y).atan2(start.x - center.x);
                        let a1 = (h.y - center.y).atan2(h.x - center.x);
                        let span = ((a1 - a0) % TAU + TAU) % TAU;
                        if span > 1e-9 {
                            let mut a = Arc::new(center, r, a0, a1);
                            a.is_counter_clockwise = true;
                            let pts = tess::arc_points(&a, tess::arc_segments(&a));
                            let scr: Vec<Pos2> =
                                pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                            paint_polyline(painter, &scr, false, Stroke::new(1.5, ACCENT));
                        }
                    }
                }
            } else if let Some(h) = snapped_hover {
                painter.line_segment(
                    [cs, self.world_to_screen(h, rect)],
                    Stroke::new(1.0, HINT_COLOR),
                );
            }
        }

        // 矩形草稿
        if let Some(c) = self.rect_corner {
            let cs = self.world_to_screen(c, rect);
            if let Some(h) = snapped_hover {
                let pts = [
                    Point::new2d(c.x, c.y),
                    Point::new2d(h.x, c.y),
                    Point::new2d(h.x, h.y),
                    Point::new2d(c.x, h.y),
                ];
                let scr: Vec<Pos2> = pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                paint_polyline(painter, &scr, true, Stroke::new(1.5, ACCENT));
            }
            paint_marker(painter, cs, ACCENT, 6.0);
        }

        // 椭圆草稿
        if let Some(c) = self.ellipse_center {
            let cs = self.world_to_screen(c, rect);
            paint_marker(painter, cs, ACCENT, 6.0);
            match (self.ellipse_major, snapped_hover) {
                (None, Some(h)) => {
                    painter.line_segment(
                        [cs, self.world_to_screen(h, rect)],
                        Stroke::new(1.0, HINT_COLOR),
                    );
                }
                (Some(m), Some(h)) => {
                    let ms = self.world_to_screen(m, rect);
                    paint_marker(painter, ms, ACCENT, 6.0);
                    painter.line_segment([cs, ms], Stroke::new(1.0, HINT_COLOR));
                    let rot = (m.y - c.y).atan2(m.x - c.x);
                    let a = c.distance_to(&m);
                    let dx = m.x - c.x;
                    let dy = m.y - c.y;
                    let len_sq = dx * dx + dy * dy;
                    let tproj = if len_sq > 1e-12 {
                        ((h.x - c.x) * dx + (h.y - c.y) * dy) / len_sq
                    } else {
                        0.0
                    };
                    let proj = Point::new2d(c.x + tproj * dx, c.y + tproj * dy);
                    let b = h.distance_to(&proj);
                    if a > 1e-9 && b > 1e-9 {
                        let e = Ellipse::new(c, a, b, rot);
                        let pts = tess::ellipse_points(&e, 96);
                        let scr: Vec<Pos2> =
                            pts.iter().map(|p| self.world_to_screen(*p, rect)).collect();
                        paint_polyline(painter, &scr, true, Stroke::new(1.5, ACCENT));
                    }
                }
                _ => {}
            }
        }

        // 填充工具预览：悬停的闭合实体
        if self.tool == Tool::Fill {
            if let Some(h) = self.hover_world {
                let cursor = self.world_to_screen(h, rect);
                if let Some(id) = self.pick_entity(cursor, rect) {
                    if let Some(target) = self.doc.get_entity(&id) {
                        let boundary: Option<Vec<Point>> = match target.geometry() {
                            EntityGeometry::Circle(c) => {
                                Some(tess::circle_points(c.center, c.radius, 72))
                            }
                            EntityGeometry::Ellipse(e) => Some(tess::ellipse_points(e, 72)),
                            EntityGeometry::Polyline(p)
                                if p.is_closed && p.vertices.len() >= 3 =>
                            {
                                Some(
                                    p.vertices
                                        .iter()
                                        .map(|v| Point::new2d(v.x, v.y))
                                        .collect(),
                                )
                            }
                            _ => None,
                        };
                        if let Some(b) = boundary {
                            if b.len() >= 3 {
                                let scr: Vec<Pos2> = b
                                    .iter()
                                    .map(|p| self.world_to_screen(*p, rect))
                                    .collect();
                                painter.add(egui::Shape::convex_polygon(
                                    scr,
                                    Color32::from_rgba_unmultiplied(
                                        self.fill_color[0],
                                        self.fill_color[1],
                                        self.fill_color[2],
                                        90,
                                    ),
                                    Stroke::NONE,
                                ));
                            }
                        }
                    }
                }
            }
        }

        // 标注草稿预览（完整标注图形，随光标更新）
        let dim_h = self.dim_text_height();
        match self.tool {
            Tool::DimLinear => {
                if !self.dim_pts.is_empty() {
                    for p in &self.dim_pts {
                        paint_marker(painter, self.world_to_screen(*p, rect), ACCENT, 6.0);
                    }
                    if let Some(hv) = snapped_hover {
                        if self.dim_pts.len() == 1 {
                            painter.line_segment(
                                [
                                    self.world_to_screen(self.dim_pts[0], rect),
                                    self.world_to_screen(hv, rect),
                                ],
                                Stroke::new(1.0, HINT_COLOR),
                            );
                        } else if let Some(entity) =
                            dim::make_linear(self.dim_pts[0], self.dim_pts[1], hv, dim_h)
                        {
                            self.paint_entity_preview(painter, &entity, rect);
                        }
                    }
                }
            }
            Tool::DimAngular => {
                if !self.dim_pts.is_empty() {
                    let v = self.dim_pts[0];
                    let vs = self.world_to_screen(v, rect);
                    paint_marker(painter, vs, ACCENT, 6.0);
                    if let Some(hv) = snapped_hover {
                        if self.dim_pts.len() == 1 {
                            painter.line_segment(
                                [vs, self.world_to_screen(hv, rect)],
                                Stroke::new(1.0, HINT_COLOR),
                            );
                        } else {
                            let r1 = self.dim_pts[1];
                            paint_marker(painter, self.world_to_screen(r1, rect), ACCENT, 6.0);
                            painter.line_segment(
                                [vs, self.world_to_screen(r1, rect)],
                                Stroke::new(1.0, HINT_COLOR),
                            );
                            if let Some(entity) = dim::make_angular(v, r1, hv, dim_h) {
                                self.paint_entity_preview(painter, &entity, rect);
                            }
                        }
                    }
                }
            }
            Tool::DimRadial | Tool::DimDiameter => {
                if let Some((center, radius)) = self.radial_target {
                    paint_marker(painter, self.world_to_screen(center, rect), ACCENT, 6.0);
                    if let Some(hv) = snapped_hover {
                        if let Some(entity) = dim::make_radial(
                            center,
                            radius,
                            hv,
                            self.tool == Tool::DimDiameter,
                            dim_h,
                        ) {
                            self.paint_entity_preview(painter, &entity, rect);
                        }
                    }
                }
            }
            _ => {}
        }

        // 编辑变换预览
        if !self.edit_pts.is_empty() {
            for p in &self.edit_pts {
                paint_marker(painter, self.world_to_screen(*p, rect), ACCENT, 6.0);
            }
            if let Some(h) = snapped_hover {
                let hs = self.world_to_screen(h, rect);
                match self.tool {
                    Tool::Move | Tool::Copy => {
                        if self.edit_pts.len() == 1 {
                            let base = self.edit_pts[0];
                            painter.line_segment(
                                [self.world_to_screen(base, rect), hs],
                                Stroke::new(1.0, HINT_COLOR),
                            );
                            let tf =
                                Transform2D::from_translation(h.x - base.x, h.y - base.y);
                            self.paint_edit_preview(painter, rect, Some(tf), None);
                        }
                    }
                    Tool::Rotate => {
                        let c = self.edit_pts[0];
                        let cs = self.world_to_screen(c, rect);
                        painter.line_segment([cs, hs], Stroke::new(1.0, HINT_COLOR));
                        if let Some(r) = self.edit_pts.get(1) {
                            let a0 = (r.y - c.y).atan2(r.x - c.x);
                            let a1 = (h.y - c.y).atan2(h.x - c.x);
                            let tf = rotate_about(c, a1 - a0);
                            self.paint_edit_preview(painter, rect, Some(tf), None);
                        }
                    }
                    Tool::Scale => {
                        let c = self.edit_pts[0];
                        let cs = self.world_to_screen(c, rect);
                        painter.line_segment([cs, hs], Stroke::new(1.0, HINT_COLOR));
                        if let Some(r) = self.edit_pts.get(1) {
                            let d0 = c.distance_to(r);
                            let d1 = c.distance_to(&h);
                            if d0 > 1e-9 && d1 > 1e-9 {
                                let tf = scale_about(c, d1 / d0);
                                self.paint_edit_preview(painter, rect, Some(tf), None);
                            }
                        }
                    }
                    Tool::Mirror => {
                        let p1 = self.edit_pts[0];
                        let p1s = self.world_to_screen(p1, rect);
                        painter.line_segment([p1s, hs], Stroke::new(1.0, HINT_COLOR));
                        if p1.distance_to(&h) > 1e-9 {
                            self.paint_edit_preview(painter, rect, None, Some((p1, h)));
                        }
                    }
                    _ => {}
                }
            }
        }

        // 框选橡皮筋
        if let Some(start) = self.band_start {
            if let Some(h) = self.hover_world {
                let band = Rect::from_two_pos(start, self.world_to_screen(h, rect));
                painter.rect_filled(
                    band,
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(86, 156, 214, 24),
                );
                painter.rect_stroke(
                    band,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
        }

        // 吸附光标标记（绘图 / 编辑工具）
        if self.tool != Tool::Select {
            if let Some((p, kind)) = self.active_snap {
                let s = self.world_to_screen(p, rect);
                paint_snap_marker(painter, s, kind);
            } else if self.snap_enabled {
                if let Some(h) = snapped_hover {
                    let s = self.world_to_screen(h, rect);
                    painter.line_segment(
                        [pos2(s.x - 5.0, s.y), pos2(s.x + 5.0, s.y)],
                        Stroke::new(1.5, ACCENT),
                    );
                    painter.line_segment(
                        [pos2(s.x, s.y - 5.0), pos2(s.x, s.y + 5.0)],
                        Stroke::new(1.5, ACCENT),
                    );
                }
            }
        }
    }

    fn ui_canvas(&mut self, ui: &mut Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = response.rect;
        if self.pending_fit {
            self.fit_view(rect);
            self.pending_fit = false;
        }
        if let Some(f) = self.pending_zoom.take() {
            self.zoom_at(rect.center(), rect, f);
        }
        self.handle_canvas_input(&response, rect);

        // 右键：绘制中 → 完成/取消；否则弹出上下文菜单
        if self.has_draft() {
            if response.secondary_clicked() {
                if self.tool == Tool::Polyline && self.poly_pts.len() >= 2 {
                    self.finish_polyline(false);
                } else if self.tool == Tool::Spline && self.poly_pts.len() >= 3 {
                    self.finish_spline();
                } else {
                    self.cancel_draft();
                    let t = self.t();
                    self.set_status(t.s_cancel_draw);
                }
            }
        } else {
            if response.secondary_clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some(id) = self.pick_entity(pos, rect) {
                        if !self.selected.contains(&id) {
                            self.selected.clear();
                            self.selected.add(id);
                        }
                    }
                }
            }
            let t = self.t();
            let has_sel = !self.selected.is_empty();
            response.context_menu(|ui| {
                if ui.button(t.mi_undo).clicked() {
                    self.undo();
                    ui.close();
                }
                if ui
                    .add_enabled(self.history.can_redo(), egui::Button::new(t.mi_redo))
                    .clicked()
                {
                    self.redo();
                    ui.close();
                }
                if ui
                    .add_enabled(self.history.can_redo(), egui::Button::new(t.mi_redo))
                    .clicked()
                {
                    self.redo();
                    ui.close();
                }
                if ui.add_enabled(has_sel, egui::Button::new(t.mi_delete)).clicked() {
                    self.delete_selected();
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_move))
                    .clicked()
                {
                    self.tool = Tool::Move;
                    ui.close();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_copy))
                    .clicked()
                {
                    self.tool = Tool::Copy;
                    ui.close();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_rotate))
                    .clicked()
                {
                    self.tool = Tool::Rotate;
                    ui.close();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_scale))
                    .clicked()
                {
                    self.tool = Tool::Scale;
                    ui.close();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_mirror))
                    .clicked()
                {
                    self.tool = Tool::Mirror;
                    ui.close();
                }
                ui.separator();
                if ui.button(t.mi_clear).clicked() {
                    self.clear_all();
                    ui.close();
                }
                if ui.button(t.mi_fit).clicked() {
                    self.pending_fit = true;
                    ui.close();
                }
                ui.toggle_value(&mut self.grid_visible, t.mi_grid_show);
                ui.toggle_value(&mut self.snap_enabled, t.mi_snap);
                ui.toggle_value(&mut self.osnap_enabled, t.mi_osnap);
                ui.toggle_value(&mut self.show_layers, t.mi_layers);
            });
        }

        self.paint_canvas(&painter, rect);
    }

    // ---------- 界面 ----------

    fn ui_menubar(&mut self, ui: &mut Ui) {
        let t = self.t();
        ui.horizontal(|ui| {
            ui.menu_button(t.m_file, |ui| {
                if ui.button(t.mi_new).clicked() {
                    self.new_document();
                }
                if ui.button(t.mi_open).clicked() {
                    self.do_open();
                }
                ui.separator();
                if ui.button(t.mi_save).clicked() {
                    self.do_save();
                }
                if ui.button(t.mi_save_as).clicked() {
                    self.do_save_as();
                }
                ui.separator();
                ui.menu_button(t.mi_import, |ui| {
                    ui.weak(t.cat_cad);
                    if ui.button("DXF…").clicked() {
                        self.do_import(ImportFormat::Dxf);
                    }
                    if ui.button("DXX…").clicked() {
                        self.do_import(ImportFormat::Dxx);
                    }
                    if ui.button("DWG…").clicked() {
                        self.do_import(ImportFormat::Dwg);
                    }
                    if ui.button("DWT…").clicked() {
                        self.do_import(ImportFormat::Dwt);
                    }
                    if ui.button("DWS…").clicked() {
                        self.do_import(ImportFormat::Dws);
                    }
                    ui.separator();
                    ui.weak(t.cat_vector);
                    if ui.button("SVG…").clicked() {
                        self.do_import(ImportFormat::Svg);
                    }
                    ui.separator();
                    ui.weak(t.cat_data);
                    if ui.button("JSON…").clicked() {
                        self.do_import(ImportFormat::Json);
                    }
                });
                ui.menu_button(t.mi_export, |ui| {
                    ui.weak(t.cat_cad);
                    if ui.button("DXF…").clicked() {
                        self.do_export_dxf();
                    }
                    ui.separator();
                    ui.weak(t.cat_vector);
                    if ui.button("SVG…").clicked() {
                        self.do_export_svg();
                    }
                    if ui.button("EPS…").clicked() {
                        self.do_export_eps();
                    }
                    if ui.button("PDF…").clicked() {
                        self.do_export_pdf();
                    }
                    if ui.button("WMF…").clicked() {
                        self.do_export_wmf();
                    }
                    ui.separator();
                    ui.weak(t.cat_bitmap);
                    if ui.button("PNG…").clicked() {
                        self.do_export_raster(raster::RasterFormat::Png);
                    }
                    if ui.button("BMP…").clicked() {
                        self.do_export_raster(raster::RasterFormat::Bmp);
                    }
                    if ui.button("JPEG…").clicked() {
                        self.do_export_raster(raster::RasterFormat::Jpeg);
                    }
                    if ui.button("WebP…").clicked() {
                        self.do_export_raster(raster::RasterFormat::WebP);
                    }
                });
                ui.separator();
                if ui.button(t.mi_exit).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button(t.m_edit, |ui| {
                if ui.button(t.mi_undo).clicked() {
                    self.undo();
                }
                if ui
                    .add_enabled(self.history.can_redo(), egui::Button::new(t.mi_redo))
                    .clicked()
                {
                    self.redo();
                }
                ui.separator();
                let has_sel = !self.selected.is_empty();
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_delete))
                    .clicked()
                {
                    self.delete_selected();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_move))
                    .clicked()
                {
                    self.tool = Tool::Move;
                    self.edit_pts.clear();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_copy))
                    .clicked()
                {
                    self.tool = Tool::Copy;
                    self.edit_pts.clear();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_rotate))
                    .clicked()
                {
                    self.tool = Tool::Rotate;
                    self.edit_pts.clear();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_scale))
                    .clicked()
                {
                    self.tool = Tool::Scale;
                    self.edit_pts.clear();
                }
                if ui
                    .add_enabled(has_sel, egui::Button::new(t.mi_mirror))
                    .clicked()
                {
                    self.tool = Tool::Mirror;
                    self.edit_pts.clear();
                }
                ui.separator();
                if ui.button(t.mi_clear).clicked() {
                    self.clear_all();
                }
            });
            ui.menu_button(t.m_view, |ui| {
                if ui.button(t.mi_fit).clicked() {
                    self.pending_fit = true;
                }
                if ui.button(t.mi_zoom_in).clicked() {
                    self.pending_zoom = Some(1.25);
                }
                if ui.button(t.mi_zoom_out).clicked() {
                    self.pending_zoom = Some(1.0 / 1.25);
                }
                if ui.button(t.mi_reset_view).clicked() {
                    self.reset_view();
                }
                ui.separator();
                ui.toggle_value(&mut self.grid_visible, t.mi_grid_show);
                ui.toggle_value(&mut self.snap_enabled, t.mi_snap);
                ui.toggle_value(&mut self.osnap_enabled, t.mi_osnap);
                ui.toggle_value(&mut self.show_layers, t.mi_layers);
            });
            ui.menu_button(t.m_settings, |ui| {
                ui.weak(t.mi_grid_size);
                for size in [1.0, 5.0, 10.0, 25.0, 50.0, 100.0] {
                    if ui
                        .selectable_label(self.grid_size == size, format!("{size:.0}"))
                        .clicked()
                    {
                        self.grid_size = size;
                    }
                }
                ui.separator();
                ui.weak(t.mi_language);
                if ui
                    .selectable_label(self.lang == Lang::Zh, "中文")
                    .clicked()
                {
                    self.lang = Lang::Zh;
                }
                if ui
                    .selectable_label(self.lang == Lang::En, "English")
                    .clicked()
                {
                    self.lang = Lang::En;
                }
            });
            ui.menu_button(t.m_help, |ui| {
                if ui.button(t.mi_shortcuts).clicked() {
                    self.show_shortcuts = true;
                }
                if ui.button(t.mi_about).clicked() {
                    self.show_about = true;
                }
            });
        });
    }

    fn ui_toolbar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let t = self.t();
        let prev_tool = self.tool;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tool, Tool::Select, t.tool_select);
            ui.separator();
            ui.selectable_value(&mut self.tool, Tool::Line, t.tool_line);
            ui.selectable_value(&mut self.tool, Tool::Polyline, t.tool_polyline);
            ui.selectable_value(&mut self.tool, Tool::Rect, t.tool_rect);
            ui.separator();
            ui.selectable_value(&mut self.tool, Tool::Circle, t.tool_circle);
            ui.selectable_value(&mut self.tool, Tool::Arc, t.tool_arc);
            ui.selectable_value(&mut self.tool, Tool::Point, t.tool_point);
            ui.selectable_value(&mut self.tool, Tool::Ellipse, t.tool_ellipse);
            ui.selectable_value(&mut self.tool, Tool::Spline, t.tool_spline);
            ui.selectable_value(&mut self.tool, Tool::Fill, t.tool_fill);
            ui.separator();
            ui.selectable_value(&mut self.tool, Tool::DimLinear, t.tool_dim_linear);
            ui.selectable_value(&mut self.tool, Tool::DimRadial, t.tool_dim_radial);
            ui.selectable_value(&mut self.tool, Tool::DimDiameter, t.tool_dim_diameter);
            ui.selectable_value(&mut self.tool, Tool::DimAngular, t.tool_dim_angular);
            if self.tool == Tool::Fill {
                ui.separator();
                ui.weak(t.tool_fill);
                ui.color_edit_button_srgb(&mut self.fill_color);
            }
        });
        if self.tool != prev_tool {
            self.cancel_draft();
            let t = self.t();
            self.set_status(t.s_tool_now.replace("{}", self.tool.label(t)));
        }
        ui.add_space(4.0);
    }

    fn ui_layers(&mut self, ui: &mut Ui) {
        let t = self.t();
        ui.heading(t.layers_title);
        ui.add_space(4.0);
        if ui.button(t.layers_add).clicked() {
            let mut idx = 1;
            loop {
                let name = format!("Layer {idx}");
                let exists = self
                    .doc
                    .layers()
                    .values()
                    .any(|l| layers::read(l).name == name);
                if !exists {
                    let layer = Layer::new(name);
                    let id = self.doc.add_layer(layer);
                    let color = LAYER_PALETTE[self.doc.layer_count() % LAYER_PALETTE.len()];
                    if let Some(l) = self.doc.layers_mut().get_mut(&id) {
                        let mut info = layers::read(l);
                        info.color = color;
                        layers::write(l, &info);
                    }
                    break;
                }
                idx += 1;
            }
        }
        ui.add_space(4.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            let ids: Vec<ObjectId> = self.doc.layers().keys().cloned().collect();
            for id in ids {
                let Some(mut info) = self.doc.get_layer(&id).map(layers::read) else {
                    continue;
                };
                let mut changed = false;
                let is_current = id == self.current_layer;
                let used = self.doc.entities().values().any(|e| e.layer_id == id);
                let is_space = id == *self.doc.model_space() || id == *self.doc.paper_space();
                ui.horizontal(|ui| {
                    let label = if is_current {
                        format!("▸ {}", info.name)
                    } else {
                        info.name.clone()
                    };
                    if ui.selectable_label(is_current, label).clicked() {
                        self.current_layer = id.clone();
                    }
                    let mut rgb = [info.color.0, info.color.1, info.color.2];
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        info.color = (rgb[0], rgb[1], rgb[2]);
                        changed = true;
                    }
                    let vis_label = if info.visible {
                        t.lay_vis
                    } else {
                        t.lay_hide
                    };
                    if ui.small_button(vis_label).clicked() {
                        info.visible = !info.visible;
                        changed = true;
                    }
                    let lock_label = if info.locked {
                        t.lay_lock
                    } else {
                        t.lay_unlock
                    };
                    if ui.small_button(lock_label).clicked() {
                        info.locked = !info.locked;
                        changed = true;
                    }
                    if ui
                        .add_enabled(
                            !used && !is_space && !is_current,
                            egui::Button::new(t.lay_del).small(),
                        )
                        .clicked()
                    {
                        self.doc.remove_layer(&id);
                    }
                });
                if changed {
                    if let Some(l) = self.doc.layers_mut().get_mut(&id) {
                        layers::write(l, &info);
                    }
                }
            }
        });
    }

    fn ui_statusbar(&self, ui: &mut Ui) {
        let t = self.t();
        ui.horizontal(|ui| {
            let coords = self
                .hover_world
                .map(|p| format!("X {:>9.2}  Y {:>9.2}", p.x, p.y))
                .unwrap_or_else(|| "X          -  Y          -".to_string());
            ui.monospace(RichText::new(coords).small());
            ui.separator();
            ui.weak(
                RichText::new(format!("{} {}", t.sb_tool, self.tool.label(t))).small(),
            );
            ui.weak(
                RichText::new(format!("{} {}", t.sb_entities, self.doc.entity_count())).small(),
            );
            ui.weak(RichText::new(format!("{} {:.0}%", t.sb_zoom, self.zoom * 100.0)).small());
            ui.weak(RichText::new(format!("{} {:.0}", t.sb_grid, self.grid_size)).small());
            if self.snap_enabled {
                ui.weak(RichText::new(t.sb_snap_on).small().color(ACCENT));
            }
            if self.osnap_enabled {
                ui.weak(RichText::new(t.sb_osnap_on).small().color(ACCENT));
            }
            // 测量信息：选区总长度 / 总面积
            let n = self.selected.count();
            if n > 0 {
                let (mut len, mut area) = (0.0, 0.0);
                for id in self.selected.entity_ids() {
                    if let Some(e) = self.doc.get_entity(id) {
                        len += entity_length(e);
                        area += entity_area(e);
                    }
                }
                ui.weak(
                    RichText::new(format!(
                        "{} {} · {} {:.2} · {} {:.2}",
                        t.sb_selected, n, t.sb_len, len, t.sb_area, area
                    ))
                    .small()
                    .color(ACCENT),
                );
            }
            ui.separator();
            // 编辑工具但无选区时提示先选择对象
            let hint = if self.tool.is_edit() && n == 0 {
                t.s_none_selected
            } else {
                self.tool.hint(t)
            };
            ui.weak(RichText::new(hint).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let file = self
                    .file_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| t.sb_unsaved.to_string());
                ui.weak(RichText::new(file).small());
                ui.separator();
                ui.label(RichText::new(&self.status).small().color(ACCENT));
            });
        });
    }

    fn ui_windows(&mut self, ctx: &Context) {
        let t = self.t();
        if self.show_about {
            egui::Window::new(t.w_about)
                .fixed_size([360.0, 200.0])
                .open(&mut self.show_about)
                .show(ctx, |ui| {
                    ui.heading("Shi — CAD");
                    ui.add_space(4.0);
                    ui.label(t.about_desc);
                    ui.add_space(6.0);
                    ui.weak(t.about_version);
                    ui.weak(t.about_feat1);
                    ui.weak(t.about_feat2);
                });
        }
        if self.show_shortcuts {
            egui::Window::new(t.w_shortcuts)
                .fixed_size([420.0, 360.0])
                .open(&mut self.show_shortcuts)
                .show(ctx, |ui| {
                    let rows: [(&str, &str); 13] = [
                        ("Ctrl+N", t.sc_new),
                        ("Ctrl+O", t.sc_open),
                        ("Ctrl+S", t.sc_save),
                        ("Ctrl+Z", t.sc_undo),
                        ("Ctrl+Y", t.sc_redo),
                        ("Delete", t.sc_del),
                        ("Esc", t.sc_esc),
                        ("F", t.sc_fit),
                        ("方向键 / Arrows", t.sc_arrows),
                        ("滚轮 / Wheel", t.sc_wheel),
                        ("中键拖动 / MMB drag", t.sc_drag),
                        ("双击 / Enter / 右键", t.sc_poly),
                        ("右键 / Right-click", t.sc_rclick),
                    ];
                    for (k, d) in rows {
                        ui.horizontal(|ui| {
                            ui.monospace(RichText::new(k).strong().small());
                            ui.label(RichText::new(d).small());
                        });
                    }
                });
        }
    }
}

fn paint_polyline(painter: &egui::Painter, pts: &[Pos2], closed: bool, stroke: Stroke) {
    if pts.len() < 2 {
        return;
    }
    for i in 0..pts.len() - 1 {
        painter.line_segment([pts[i], pts[i + 1]], stroke);
    }
    if closed {
        painter.line_segment([pts[pts.len() - 1], pts[0]], stroke);
    }
}

fn paint_marker(painter: &egui::Painter, p: Pos2, color: Color32, size: f32) {
    painter.rect_filled(
        Rect::from_center_size(p, Vec2::splat(size)),
        CornerRadius::ZERO,
        color,
    );
}

/// 对象捕捉标记：端点=方块 中点=三角 圆心=圆 交点=叉
fn paint_snap_marker(painter: &egui::Painter, p: Pos2, kind: SnapKind) {
    let stroke = Stroke::new(1.5, ACCENT);
    match kind {
        SnapKind::EndPoint => {
            painter.rect_stroke(
                Rect::from_center_size(p, Vec2::splat(8.0)),
                CornerRadius::ZERO,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        SnapKind::MidPoint => {
            let (x, y) = (p.x, p.y);
            painter.line_segment([pos2(x - 5.0, y + 3.0), pos2(x + 5.0, y + 3.0)], stroke);
            painter.line_segment([pos2(x - 5.0, y + 3.0), pos2(x, y - 4.0)], stroke);
            painter.line_segment([pos2(x + 5.0, y + 3.0), pos2(x, y - 4.0)], stroke);
        }
        SnapKind::Center => {
            painter.circle(p, 5.0, Color32::TRANSPARENT, stroke);
        }
        SnapKind::Intersection => {
            painter.line_segment([pos2(p.x - 5.0, p.y - 5.0), pos2(p.x + 5.0, p.y + 5.0)], stroke);
            painter.line_segment([pos2(p.x - 5.0, p.y + 5.0), pos2(p.x + 5.0, p.y - 5.0)], stroke);
        }
    }
}

fn dist_point_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// 叉积（用于线段相交判断）
fn cross_z(o: Pos2, a: Pos2, b: Pos2) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

fn segments_intersect(p1: Pos2, p2: Pos2, p3: Pos2, p4: Pos2) -> bool {
    let d1 = cross_z(p3, p4, p1);
    let d2 = cross_z(p3, p4, p2);
    let d3 = cross_z(p1, p2, p3);
    let d4 = cross_z(p1, p2, p4);
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

/// 线段是否与矩形相交（端点在矩形内或跨越矩形边界）
fn seg_hits_rect(a: Pos2, b: Pos2, r: Rect) -> bool {
    if a.x.max(b.x) < r.left()
        || a.x.min(b.x) > r.right()
        || a.y.max(b.y) < r.top()
        || a.y.min(b.y) > r.bottom()
    {
        return false;
    }
    if r.contains(a) || r.contains(b) {
        return true;
    }
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    for i in 0..4 {
        if segments_intersect(a, b, corners[i], corners[(i + 1) % 4]) {
            return true;
        }
    }
    false
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        Self::apply_theme(&ctx);
        self.handle_shortcuts(&ctx);

        egui::Panel::top("menubar").show(root, |ui| {
            self.ui_menubar(ui);
        });

        egui::Panel::top("toolbar").show(root, |ui| {
            self.ui_toolbar(ui);
        });

        if self.show_layers {
            egui::Panel::left("layers")
                .resizable(true)
                .default_size(220.0)
                .min_size(160.0)
                .show(root, |ui| {
                    self.ui_layers(ui);
                });
        }

        egui::Panel::bottom("statusbar").show(root, |ui| {
            self.ui_statusbar(ui);
        });

        egui::CentralPanel::no_frame().show(root, |ui| {
            self.ui_canvas(ui);
        });

        self.ui_windows(&ctx);
    }
}
