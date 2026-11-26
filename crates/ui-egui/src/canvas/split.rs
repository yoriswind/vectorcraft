//! Two render modes on one native canvas and one tool host.
use craft::behavior::{Axis, CancelReason, CanvasView, Metrics, PartKey, PointerEvent, PressPhase, Sense};
use craft::ui::canvas::{CanvasSpace, RasterDisplay, AppPaint};
use craft::ui::primitives::ContentPainterCx;
use craft::behavior::PixelSize;
use craft::ui::contract::{Access, Bind, BindMut, Common, Component, Cx, Edit as _, EditResponse, PaintUi, PartsCx, Pointer as _, Props, Role, Show as _};
use craft::ui::docking::{divider, Splitter};
use craft::ui::runtime::{env::Env, pointer};
use craft::ui::style::{ComponentStyle, StyleCx};
use egui::{Pos2, Ui};
use vectorcraft_engine::ViewInfo;
use crate::{VectorcraftApp, doc_windows::{self, Win}, state::{SplitMode, SplitOrientation, SplitView}};
use super::raster;

const EDGE: PartKey = PartKey::new("comparison-divider");
#[derive(Clone, Copy)]
struct Motion { axis: f32, moved: bool }
#[derive(Clone, Copy)]
struct HeldMode { win: Win, doc:u64, outline: bool, pixel: bool, active:bool }
fn held_id() -> egui::Id { egui::Id::unique("split-held-mode") }

pub(super) fn enabled(app: &VectorcraftApp) -> bool {
    app.ui.screen_mode < 3 && !crate::menus::home_showing(app) && app.view().is_some_and(|v| v.split.enabled) && app.session.active().is_some()
}
pub(super) fn suspend(app: &mut VectorcraftApp, ctx: &egui::Context) {
    if craft::ui::runtime::measure::active(ctx) {return;}
    if !enabled(app) { app.canvas.split.clear(); }
}

pub(super) fn clips(rect: egui::Rect, split: SplitView) -> [egui::Rect; 2] {
    let (mut a, mut b) = (rect, rect);
    match split.orientation {
        SplitOrientation::Vertical => { let x = rect.left() + rect.width() * split.ratio(); a.max.x=x; b.min.x=x; }
        SplitOrientation::Horizontal => { let y = rect.top() + rect.height() * split.ratio(); a.max.y=y; b.min.y=y; }
    }
    [a,b]
}
fn axis(orientation: SplitOrientation) -> Axis { if orientation == SplitOrientation::Vertical { Axis::X } else { Axis::Y } }
fn coord(orientation: SplitOrientation, p: Pos2) -> f32 { if orientation == SplitOrientation::Vertical { p.x } else { p.y } }

#[derive(Clone, Debug, PartialEq, ComponentStyle)]
struct DividerStyle { #[style(metrics)] metrics: Metrics }
#[derive(Component, Props)]
#[component(canvas_control)]
struct ComparisonDivider<'a> {
    #[common] common: Common,
    fraction: Bind<'a, f32>,
    orientation: SplitOrientation,
    admit: bool,
}
impl ComparisonDivider<'_> {
    fn line(&self, rect: egui::Rect, fraction: f32, rtl:bool) -> divider::Divider {
        let (lo,hi) = if self.orientation == SplitOrientation::Vertical { (rect.left(),rect.right()) } else { (rect.top(),rect.bottom()) };
        divider::Divider::new(axis(self.orientation), if rtl && self.orientation==SplitOrientation::Vertical {hi-(hi-lo)*fraction} else {lo+(hi-lo)*fraction}, rect, (lo,hi), 6.0)
    }
    fn ratio(&self, rect: egui::Rect, at: f32, rtl:bool) -> f32 {
        let (lo,extent) = if self.orientation == SplitOrientation::Vertical { (rect.left(),rect.width()) } else { (rect.top(),rect.height()) };
        let ratio=((at-lo)/extent.max(1.0)).clamp(0.0,1.0);
        if rtl && self.orientation==SplitOrientation::Vertical {1.0-ratio} else {ratio}
    }
}
impl Component for ComparisonDivider<'_> {
    const ROLE: Role = Role::GROUP;
    const KEYS: craft::ui::runtime::focus::KeyUse = craft::ui::runtime::focus::KeyUse::SPLITTER;
    type Style = DividerStyle;
    type Output = EditResponse<f32>;
    fn style(_: &StyleCx<'_>, _: ComparisonDividerStyleKey) -> DividerStyle { DividerStyle { metrics: Metrics::new() } }
    fn sense(&self) -> Sense { Sense::empty() }
    fn bind(&mut self) -> Option<BindMut<'_, f32>> { Some(self.fraction.reborrow()) }
    fn parts(&mut self, rect: egui::Rect, cx: &mut PartsCx<'_, Self>) {
        let line = self.line(rect, cx.value().copied().unwrap_or(0.5),cx.flow.is_rtl());
        let part=divider::declare(cx, rect, EDGE, 0, &line);
        if !self.admit {part.sense(Sense::HOVER);}
    }
    fn update(&mut self, cx: &mut Cx<'_, Self>) {
        let phase = cx.part(EDGE.at(0)).press.phase;
        let id = cx.id.with("axis-motion");
        if let PressPhase::Down { start, .. } = phase {
            cx.ctx.data_mut(|d| d.insert_temp(id, Motion { axis: coord(self.orientation,start.origin), moved:false }));
            cx.begin();
        }
        if let Some(mut motion) = cx.ctx.data(|d| d.get_temp::<Motion>(id)) {
            // H0 admits the Down last. Its earlier hover samples are not this owner's motion.
            if !matches!(phase, PressPhase::Down { .. }) {
                motion.moved |= pointer::with_pass_pointer(&cx.ctx, |pass| pass.events.iter().take_while(|e| !matches!(e, PointerEvent::Pressed { .. })).any(|e| {
                    matches!(e, PointerEvent::Moved { at, .. } if coord(self.orientation,*at) != motion.axis)
                }));
                let at = match phase {
                    PressPhase::DragStart { pos, .. } | PressPhase::Drag { pos, .. } | PressPhase::Release { pos, .. } => Some(pos),
                    PressPhase::Held { .. } => pointer::frame(&cx.ctx).pos,
                    _ => None,
                };
                motion.moved |= at.is_some_and(|at|coord(self.orientation,at)!=motion.axis);
                if motion.moved && let Some(at) = at { cx.propose(self.ratio(cx.rect,coord(self.orientation,at),false)); }
            }
            match phase {
                PressPhase::Release { .. } => { cx.commit(); cx.ctx.data_mut(|d| d.remove::<Motion>(id)); }
                PressPhase::Cancel { .. } => { cx.cancel(CancelReason::Escape); cx.ctx.data_mut(|d| d.remove::<Motion>(id)); }
                _ => { cx.ctx.data_mut(|d| d.insert_temp(id,motion)); }
            }
        }
        let line = self.line(cx.rect,cx.value().copied().unwrap_or(0.5),cx.flow.is_rtl());
        if let Some((_,at)) = divider::keys(cx,EDGE,&[(0,line)],1.0,10.0) { cx.set(self.ratio(cx.rect,at,cx.flow.is_rtl())); }
    }
    fn paint_over(&mut self, ui: &mut PaintUi<'_>, _: &Cx<'_, Self>) { ui.face(EDGE.at(0),Splitter::new(axis(self.orientation))); }
    fn access(&self, access: &mut Access, cx: &Cx<'_, Self>) {
        let line=self.line(cx.rect,cx.value().copied().unwrap_or(0.5),cx.flow.is_rtl());
        divider::describe(access,EDGE,0,&line,tl!("Split View divider"),1.0);
    }
    fn finish(self, cx: Cx<'_,Self>) -> EditResponse<f32> { cx.edit_response() }
}

pub(super) fn control(app: &mut VectorcraftApp, ui: &mut Ui, space: &CanvasSpace, win: Win) {
    if !enabled(app) || craft::ui::runtime::measure::active(ui.ctx()) { return; }
    let Some(mut split) = doc_windows::view(app,win).map(|v| v.split) else { return };
    let id=super::canvas_id(win).with(("comparison",split.orientation == SplitOrientation::Horizontal));
    let owns=ui.ctx().data(|d|d.get_temp::<Motion>(id.with("axis-motion")).is_some());
    let (space,middle,mods)=ui.input(|i|(i.key_down(egui::Key::Space),i.pointer.middle_down(),i.modifiers));
    // A selected Hand/Zoom tool still yields its seam press to the divider. Space lends navigation;
    // while Type edits text only Cmd+Space does, as on the ordinary canvas.
    let view_tool=space && (!app.session.tool_wants_text() || mods.command);
    let admit=owns || !middle && !view_tool;
    let shown = ComparisonDivider { common:Common::new(), fraction:Bind::bound(&mut split.fraction), orientation:split.orientation,admit }
        .label(tl!("Split View divider")).id(id)
        .show_at(ui,space.view.rect);
    let _ = shown;
    if let Some(view) = doc_windows::view_mut(app,win) { view.split.fraction=split.ratio(); }
}

/// Mode policy shared by the tool host, hover consumers and point-based canvas picking.
pub(super) fn view_at(app:&VectorcraftApp,ctx:&egui::Context,at:Option<Pos2>) -> ViewInfo {
    let mut view=app.view_info();
    let Some(win)=doc_windows::current(app) else{return view};
    if let Some(mut held)=ctx.data(|d|d.get_temp::<HeldMode>(held_id())).filter(|m|m.win==win && app.session.active().is_some_and(|st|st.uid==m.doc)) {
        if held.active || enabled(app) {
            held.active=true;
            view.outline=held.outline;view.pixel_preview=held.pixel;
        } else {
            // With Split View off, ordinary mode changes retain main's behavior. Remember the
            // latest ordinary mode only so enabling comparison cannot take over an existing press.
            held.outline=view.outline;held.pixel=view.pixel_preview;
        }
        if !craft::ui::runtime::measure::active(ctx) {ctx.data_mut(|d|d.insert_temp(held_id(),held));}
    } else if enabled(app) && let (Some(v),Some(canvas),Some(at))=(app.view(),app.canvas_view(),at) {
        if clips(canvas.rect,v.split)[1].contains(at) {view.outline=v.split.comparison==SplitMode::Outline;view.pixel_preview=v.split.comparison==SplitMode::PixelPreview;}
    }
    view
}
pub(super) fn view_doc(app:&VectorcraftApp,at:vectorcraft_geom::Point) -> ViewInfo {
    let Some(ctx)=app.ui_ctx.as_ref() else{return app.view_info()};
    view_at(app,ctx,app.canvas_view().map(|view|view.to_screen(at)))
}
pub(super) fn input_view(app: &VectorcraftApp, ctx: &egui::Context, pass: &craft::ui::canvas::ToolPass) -> ViewInfo {
    let start=pass.press.phase.start();
    let at=start.map(|s|s.origin).or_else(||pointer::frame(ctx).pos);
    let view=view_at(app,ctx,at);
    if start.is_some() && pass.press.phase.is_press() && let (Some(win),Some(st))=(doc_windows::current(app),app.session.active()) {
        let held=ctx.data(|d|d.get_temp::<HeldMode>(held_id())).is_some_and(|m|m.win==win && m.doc==st.uid);
        if !held {ctx.data_mut(|d|d.insert_temp(held_id(),HeldMode {win,doc:st.uid,outline:view.outline,pixel:view.pixel_preview,active:enabled(app)}));}
    }
    view
}
pub(super) fn end_pass(ctx:&egui::Context,pass:&craft::ui::canvas::ToolPass) {
    if matches!(pass.press.phase,PressPhase::Release {..}|PressPhase::Cancel {..}) {ctx.data_mut(|d|d.remove::<HeldMode>(held_id()));}
}
#[derive(Clone, Copy, PartialEq)]
struct ModeContext {
    doc: u64,
    outline: bool,
    pixel: bool,
    trim: bool,
    overprint: bool,
    comparison: SplitMode,
}

fn mode_id() -> egui::Id { egui::Id::unique("split-mode-context") }
pub(super) fn draw_art(app: &mut VectorcraftApp, ui: &mut Ui, space: &CanvasSpace) {
    if craft::ui::runtime::measure::active(ui.ctx()) {return;}
    let Some(v) = app.view().copied() else { return };
    let view=space.view;
    let rect=view.rect;
    let placed_generation = vectorcraft_render::placed_document::generation();
    let Some(st) = app.session.active() else { return };
    let mode = ModeContext {
        doc: st.uid,
        outline: v.outline,
        pixel: app.ui.view.pixel_preview,
        trim: app.ui.view.trim_view,
        overprint: vectorcraft_render::proof::overprint_preview_on(),
        comparison: v.split.comparison,
    };
    let mode_changed = ui.data_mut(|d| {
        let old = d.get_temp::<ModeContext>(mode_id());
        d.insert_temp(mode_id(), mode);
        old.is_some_and(|old| old != mode)
    });
    // Pixel Preview can have the same raster options below 100%; its logical mode still owns
    // a distinct pair context. Camera/pixel-region movement deliberately leaves this stamp alone.
    if mode_changed {
        app.canvas.split.clear();
    }
    let Some(first) = job(app, ui, &view, None) else { return };
    let Some(mut second) = job(app, ui, &view, Some(v.split.comparison)) else { return };
    // These asynchronous resources can advance between the two job preparations. Bind both
    // sides to one recorded generation/proof snapshot; the next frame observes any later one.
    second.key.plugins = first.key.plugins;
    second.key.placed = first.key.placed;
    second.opts.proof = first.opts.proof.clone();
    second.key.color=first.key.color.clone();
    let report_doc=first.doc.clone();
    let report_effects=first.effects.clone();
    // Single-view results must still be drained while split owns the screen, so completed jobs
    // cannot keep an old document alive indefinitely or upload an unused texture later.
    for worker in [&mut app.canvas.worker, &mut app.canvas.fill_worker].into_iter().flatten() {
        let _ = worker.poll();
    }
    app.canvas.texture = None;
    app.canvas.key = None;
    app.canvas.shown = None;
    app.canvas.fill = None;
    app.canvas.split.update(ui.ctx(), [first, second]);
    if let Some(pair) = app.canvas.split.shown() {
        app.perf.render_ms = pair.ms;
        let (pw,ph)=pair.keys[0].pixel.map_or((pair.keys[0].w,pair.keys[0].h),|[x0,y0,x1,y1]|((x1-x0) as u32,(y1-y0) as u32));
        let paint = AppPaint::new(|cx:&ContentPainterCx<'_>, at:CanvasView| {
            for (side,clip) in clips(at.rect,v.split).into_iter().enumerate() {
                if let (Some(texture),Some(key)) = (pair.textures.get(side),pair.keys.get(side)) {
                    raster::reproject_texture(&cx.painter.with_clip_rect(clip), &at, at.rect, texture, key, v.rotation);
                }
            }
        });
        RasterDisplay::painted(tl!("Artwork comparison"),PixelSize::new(pw.max(1),ph.max(1)),paint).picture().show_at(ui,rect);
        // Retire only the document whose complete pair was drawn, never a merely queued view.
        if let Some(st) = app.session.active().filter(|st| pair.keys.iter().all(|key| key.doc == st.uid as usize)) {
            st.plugin_effects.first_view_done();
        }
    }
    raster::report_rendered(app,&report_doc,&report_effects);
    // Bitmaps can start and finish while a synchronous pair is drawn. Their generation changes
    // before busy becomes false, so also wake for a completion since the pair was prepared.
    if vectorcraft_render::placed_document::busy() || vectorcraft_render::placed_document::generation() != placed_generation {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
    }
}

fn job(app: &mut VectorcraftApp, ui: &Ui, drawn: &CanvasView, mode: Option<SplitMode>) -> Option<crate::render_worker::Job> {
    let st = app.session.active()?;
    let doc = st.doc.clone();
    let effects = st.plugin_effects.clone();
    let mask_view = st.shown_mask();
    let rect=drawn.rect;
    let t=Env::of(ui);
    let device_ppp = ui.ctx().pixels_per_point();
    let max_side = ui.ctx().input(|i| i.max_texture_side);
    let ppp = raster::frame_ppp(rect.size(), device_ppp, max_side);
    let (w, h) = (
        ((rect.width() * ppp).round().max(1.0) as u32).min(u32::from(u16::MAX)),
        ((rect.height() * ppp).round().max(1.0) as u32).min(u32::from(u16::MAX)),
    );
    let pixel_mode = mode.map_or(app.ui.view.pixel_preview, |m| m == SplitMode::PixelPreview);
    let pixel = (pixel_mode && drawn.zoom * f64::from(device_ppp) > 1.0).then(|| raster::pixel_region(drawn, max_side)).flatten();
    let smooth = pixel.is_none() || app.session.prefs.anti_aliased_bitmaps;
    let plugins = raster::plugin_key(&mut app.canvas.plugin_keys, &app.session, st);
    let placed = raster::placed_key(&mut app.canvas.placed_keys, &app.session, st);
    let mut key = raster::cache_key(app, ui.ctx(), st, t.tokens.tokens(), drawn, (w, h, ppp), (pixel, smooth, plugins, placed));
    if let Some(mode) = mode {
        key.outline = mode == SplitMode::Outline;
        key.trim = mode == SplitMode::TrimView;
        key.backdrop = raster::backdrop_mode(app, st, key.outline,mode==SplitMode::OverprintPreview);
    }
    let mut opts = raster::frame_options(app, &key, mask_view, smooth);
    if let Some(mode) = mode {
        opts.outline = mode == SplitMode::Outline;
        opts.trim = mode == SplitMode::TrimView;
        opts.overprint_preview = mode == SplitMode::OverprintPreview;
    }
    let (w, h, view) = raster::frame_view(pixel, (w, h), drawn, ppp);
    Some(crate::render_worker::Job {
        key,
        doc,
        w,
        h,
        view,
        opts,
        scope: vectorcraft_engine::file_access::ScopeHandle::capture(),
        effects,
        may_copy: false,
        may_keep: false,
        display: app.display_profile.transform(app.session.prefs.monitor_profile_display),
    })
}


/// One name/rename owner, with mode-specific native paper through the common picture slot.
pub(super) fn paper(app:&VectorcraftApp,ui:&mut Ui,space:&CanvasSpace,win:Win) {
    use craft::ui::canvas::{Pasteboard,PageBorder,ArtboardEdge,PasteboardColor,with_canvas_space};
    use craft::ui::style::{DataColor,StyleCx};
    let (Some(st),Some(view))=(app.session.documents().get(win.0),doc_windows::view(app,win)) else{return};
    for (side,clip) in clips(space.view.rect,view.split).into_iter().enumerate() {
        let outline=if side==0 {view.outline} else {view.split.comparison==SplitMode::Outline};
        let overprint=if side==0 {vectorcraft_render::proof::overprint_preview_on()} else {view.split.comparison==SplitMode::OverprintPreview};
        let env=Env::of(ui);
        let color=if outline || overprint {egui::Color32::WHITE} else if !super::artboards::shows_artboards(app) {
            super::artboards::paper_color(&st.doc.setup)
        } else {crate::prefs_dialog::canvas_color(&app.session.prefs.canvas_color)
            .unwrap_or_else(||StyleCx::of_env(ui.ctx(),&env).color(env.tokens.color.surface.canvas).egui())};
        let mut child=ui.new_child(egui::UiBuilder::new().max_rect(space.view.rect).id_salt(("split-paper",side)));
        child.set_clip_rect(ui.clip_rect().intersect(clip));
        let mut pages=super::artboards::frames(app,st,view,outline);
        for page in &mut pages {page.name=None;page.renames=false;page.active=None;}
        let border=if outline {PageBorder::Line} else if overprint {PageBorder::None} else {PageBorder::DropShadow};
        with_canvas_space(&mut child,space.with_pasteboard(PasteboardColor::Custom(DataColor::new(color))),|ui| {
            Pasteboard::new(tl!("Comparison artboards"),&pages).border(border).artboard_edge(ArtboardEdge::Plain).shows_outside().show_at(ui,space.view.rect);
        });
    }
}

pub(super) fn pixel_grids(app:&VectorcraftApp,ui:&mut Ui,space:&CanvasSpace,win:Win) {
    let Some(view)=doc_windows::view(app,win) else{return};
    for (side,clip) in clips(space.view.rect,view.split).into_iter().enumerate() {
        let pixel=if side==0 {app.ui.view.pixel_preview} else {view.split.comparison==SplitMode::PixelPreview};
        if let Some(grid)=super::grid::pixel_grid_mode(app,app.print_ppi_of(Some(win)),pixel) {
            let mut child=ui.new_child(egui::UiBuilder::new().max_rect(space.view.rect).id_salt(("comparison-pixel-grid",side)));
            child.set_clip_rect(ui.clip_rect().intersect(clip));
            grid.show_at(&mut child,space.view.rect);
        }
    }
}
