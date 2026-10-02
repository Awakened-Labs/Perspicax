//! Pictures of the screen, drawn when one is asked for and never otherwise.
//!
//! A seat draws them with the GPU it composites with; headless, which draws
//! nothing for anyone, draws them in software with pixman. Either way the
//! picture is made the same way: the same render elements a seat's frame is
//! made of, rendered into a buffer of our own instead of onto a monitor, and
//! read back.
//!
//! What goes in is decided before anything is drawn ([`Plan`]), and that is
//! where the provenance comes from: every surface in the picture, where it
//! is, and who drew it, read from the facts this compositor published. A
//! window the agent holds no consent for is not drawn at all. A solid block
//! is drawn where it would be, and the account says so. A picture that
//! showed it would be an agent reading, through pixels, a window the gate
//! would refuse it.
//!
//! Without the `capture` feature, every picture is refused as not built.

use perspicax_index::{Shot, ShotTarget};

use crate::{act::ActError, state::Compositor};

impl Compositor {
    /// Take a picture. Refused while the session is locked: the person has
    /// walked away, and nothing reads their screen until they are back.
    pub(crate) fn capture(&mut self, target: &ShotTarget) -> Result<Shot, ActError> {
        if self.lock.is_some() {
            return Err(ActError::Locked);
        }
        #[cfg(feature = "capture")]
        return taking::take(self, target);
        #[cfg(not(feature = "capture"))]
        {
            let _ = target;
            Err(ActError::NotBuilt("capture"))
        }
    }
}

#[cfg(feature = "capture")]
pub(crate) use taking::{Pixels, draw_output};

#[cfg(feature = "capture")]
mod taking {
    use perspicax_index::{Drawn, Shot, ShotTarget};
    use perspicax_node::{Origin, Rect, SurfaceId};
    use smithay::{
        backend::{
            allocator::Fourcc,
            renderer::{
                Bind, ExportMem, ImportAll, ImportMem, Offscreen, Renderer, Texture,
                damage::OutputDamageTracker,
                element::{
                    AsRenderElements, Kind,
                    solid::{SolidColorBuffer, SolidColorRenderElement},
                    surface::WaylandSurfaceRenderElement,
                },
            },
        },
        desktop::{LayerSurface, layer_map_for_output, space::SpaceElement},
        utils::{
            Buffer as BufferCoord, Logical, Physical, Point, Rectangle, Scale, Size, Transform,
        },
        wayland::shell::wlr_layer::Layer,
    };

    use crate::{
        BACKDROP, act::ActError, backend::Running, framed::Framed, framed::FramedElement,
        shell::id_of, state::Compositor,
    };

    smithay::backend::renderer::element::render_elements! {
        /// What a picture is made of: windows with their frames, layer
        /// surfaces, and the blocks painted over what may not be shown.
        pub(crate) Scene<R> where R: ImportAll + ImportMem;
        Window=FramedElement<R>,
        Surface=WaylandSurfaceRenderElement<R>,
        Solid=SolidColorRenderElement,
    }

    /// What is painted over a window the agent may not see: opaque, and
    /// unlike anything a window draws, so nobody mistakes it for content.
    const REDACTED: [f32; 4] = [0.35, 0.35, 0.38, 1.0];

    /// A picture's pixels: top row first, RGBA.
    pub(crate) struct Pixels {
        pub(crate) width: u32,
        pub(crate) height: u32,
        pub(crate) rgba: Vec<u8>,
    }

    /// What will be drawn, and in what order, before the renderer is touched.
    struct Plan {
        size: Size<i32, Physical>,
        scale: f64,
        output: Option<String>,
        /// Front to back: panels over the windows, then the windows, then the
        /// wallpaper beneath them.
        upper: Vec<(LayerSurface, Point<i32, Logical>)>,
        windows: Vec<Placed>,
        lower: Vec<(LayerSurface, Point<i32, Logical>)>,
        drawn: Vec<Drawn>,
        redacted: Vec<Drawn>,
    }

    /// A window, where it is drawn from, and whether it is painted over.
    struct Placed {
        window: Framed,
        /// Where the window's surface origin goes, in the picture.
        at: Point<i32, Logical>,
        /// Its whole extent, frame included, in the picture.
        cover: Rectangle<i32, Logical>,
        redacted: bool,
    }

    pub(super) fn take(state: &mut Compositor, target: &ShotTarget) -> Result<Shot, ActError> {
        let plan = match target {
            ShotTarget::Output(name) => plan_output(state, name.as_deref())?,
            ShotTarget::Window(id) => plan_window(state, *id)?,
        };
        let pixels = render(state, &plan)?;
        Ok(Shot {
            width: pixels.width,
            height: pixels.height,
            scale: plan.scale,
            output: plan.output,
            rgba: pixels.rgba,
            drawn: plan.drawn,
            redacted: plan.redacted,
        })
    }

    /// Draw a monitor as it looks, with nothing painted over: what the
    /// person's own screenshot tool is given. Consent is the agent's, and
    /// this is not the agent asking.
    pub(crate) fn draw_output(state: &mut Compositor, output: &str) -> Result<Pixels, ActError> {
        let mut plan = plan_output(state, Some(output))?;
        for placed in &mut plan.windows {
            placed.redacted = false;
        }
        render(state, &plan)
    }

    /// Who drew a surface, from what was last published, and whether the
    /// agent may see it.
    fn origin(state: &Compositor, id: SurfaceId) -> (Origin, bool) {
        let origin = state
            .facts
            .read()
            .surface(id)
            .map(|facts| facts.origin.clone())
            .unwrap_or_default();
        let permitted = state.consent.permits(&origin);
        (origin, permitted)
    }

    fn logical(rect: Rectangle<i32, Logical>) -> Rect {
        Rect::new(
            f64::from(rect.loc.x),
            f64::from(rect.loc.y),
            f64::from(rect.loc.x + rect.size.w),
            f64::from(rect.loc.y + rect.size.h),
        )
    }

    fn plan_output(state: &Compositor, name: Option<&str>) -> Result<Plan, ActError> {
        let space = &state.space;
        let output = match name {
            Some(name) => space.outputs().find(|output| output.name() == name),
            None => space.outputs().next(),
        }
        .cloned()
        .ok_or_else(|| ActError::NoSuchOutput(name.unwrap_or("(any)").to_owned()))?;
        let area = space
            .output_geometry(&output)
            .ok_or_else(|| ActError::NoSuchOutput(output.name()))?;
        let scale = output.current_scale().fractional_scale();
        let size = output
            .current_mode()
            .map(|mode| mode.size)
            .ok_or_else(|| ActError::NoSuchOutput(output.name()))?;

        let mut drawn = Vec::new();
        let mut redacted = Vec::new();
        let mut account = |id: Option<SurfaceId>, rect: Rectangle<i32, Logical>, hidden: bool| {
            let Some(id) = id else { return };
            let (origin, _) = origin(state, id);
            let entry = Drawn {
                surface: id,
                rect: logical(rect),
                origin,
            };
            if hidden {
                redacted.push(entry);
            } else {
                drawn.push(entry);
            }
        };

        let layers = layer_map_for_output(&output);
        let mut upper = Vec::new();
        let mut lower = Vec::new();
        for layer in layers.layers().rev() {
            let Some(placed) = layers.layer_geometry(layer) else {
                continue;
            };
            let id = layer.user_data().get::<SurfaceId>().copied();
            match layer.layer() {
                Layer::Top | Layer::Overlay => {
                    account(id, placed, false);
                    upper.push((layer.clone(), placed.loc));
                }
                Layer::Background | Layer::Bottom => lower.push((layer.clone(), placed.loc)),
            }
        }
        drop(layers);

        let mut windows = Vec::new();
        for window in space.elements().rev() {
            let Some(bbox) = space.element_bbox(window) else {
                continue;
            };
            if !bbox.overlaps(area) {
                continue;
            }
            let Some(location) = space.element_location(window) else {
                continue;
            };
            let id = id_of(window);
            let hidden = id.is_some_and(|id| !origin(state, id).1);
            let client = Rectangle::new(location - area.loc, window.geometry().size);
            account(id, client, hidden);
            windows.push(Placed {
                window: window.clone(),
                at: location - window.geometry().loc - area.loc,
                cover: Rectangle::new(bbox.loc - area.loc, bbox.size),
                redacted: hidden,
            });
        }

        let layers = layer_map_for_output(&output);
        for (layer, at) in &lower {
            let id = layer.user_data().get::<SurfaceId>().copied();
            if let Some(placed) = layers.layer_geometry(layer) {
                account(id, Rectangle::new(*at, placed.size), false);
            }
        }
        drop(layers);

        Ok(Plan {
            size,
            scale,
            output: Some(output.name()),
            upper,
            windows,
            lower,
            drawn,
            redacted,
        })
    }

    fn plan_window(state: &Compositor, id: SurfaceId) -> Result<Plan, ActError> {
        let window = state.any_window(id).ok_or(ActError::NoSuchSurface(id.0))?;
        let (origin, permitted) = origin(state, id);
        // Refused rather than painted over: a picture of nothing but the
        // block would only say that it was refused.
        if !permitted {
            return Err(ActError::Capture(
                "the agent holds no consent for this window's process".to_owned(),
            ));
        }
        let bbox = SpaceElement::bbox(&window);
        if bbox.size.w <= 0 || bbox.size.h <= 0 {
            return Err(ActError::Capture(
                "the window has drawn nothing yet".to_owned(),
            ));
        }
        let geometry = window.geometry();
        let at = Point::from((0, 0)) - bbox.loc;
        Ok(Plan {
            size: (bbox.size.w, bbox.size.h).into(),
            scale: 1.0,
            output: None,
            upper: Vec::new(),
            windows: vec![Placed {
                window,
                at,
                cover: Rectangle::from_size(bbox.size),
                redacted: false,
            }],
            lower: Vec::new(),
            drawn: vec![Drawn {
                surface: id,
                rect: logical(Rectangle::new(geometry.loc - bbox.loc, geometry.size)),
                origin,
            }],
            redacted: Vec::new(),
        })
    }

    /// Draw a plan with whichever renderer this backend has.
    fn render(state: &mut Compositor, plan: &Plan) -> Result<Pixels, ActError> {
        let failed = |error: String| ActError::Capture(error);
        match &mut state.backend {
            Running::Headless { pixman, .. } => {
                let renderer = match pixman {
                    Some(renderer) => renderer,
                    None => pixman.insert(Box::new(
                        smithay::backend::renderer::pixman::PixmanRenderer::new()
                            .map_err(|error| failed(error.to_string()))?,
                    )),
                };
                draw::<_, smithay::reexports::pixman::Image<'static, 'static>>(
                    &mut **renderer,
                    plan,
                )
            }
            #[cfg(feature = "seat")]
            Running::Seat(session) => {
                draw::<_, smithay::backend::renderer::gles::GlesTexture>(session.renderer(), plan)
            }
        }
    }

    fn draw<R, T>(renderer: &mut R, plan: &Plan) -> Result<Pixels, ActError>
    where
        R: Renderer + ImportAll + ImportMem + Offscreen<T> + Bind<T> + ExportMem,
        R::TextureId: Texture + Send + Clone + 'static,
    {
        let failed = |error: &dyn std::fmt::Display| ActError::Capture(error.to_string());
        let scale = Scale::from(plan.scale);
        let mut elements: Vec<Scene<R>> = Vec::new();
        let layer = |renderer: &mut R, layer: &LayerSurface, at: Point<i32, Logical>| {
            AsRenderElements::<R>::render_elements::<WaylandSurfaceRenderElement<R>>(
                layer,
                renderer,
                at.to_physical_precise_round(scale),
                scale,
                1.0,
            )
        };
        for (surface, at) in &plan.upper {
            elements.extend(
                layer(renderer, surface, *at)
                    .into_iter()
                    .map(Scene::Surface),
            );
        }
        for placed in &plan.windows {
            if placed.redacted {
                let block = SolidColorBuffer::new(placed.cover.size, REDACTED);
                elements.push(Scene::Solid(SolidColorRenderElement::from_buffer(
                    &block,
                    placed.cover.loc.to_physical_precise_round(scale),
                    scale,
                    1.0,
                    Kind::Unspecified,
                )));
            } else {
                elements.extend(
                    placed
                        .window
                        .render_elements::<FramedElement<R>>(
                            renderer,
                            placed.at.to_physical_precise_round(scale),
                            scale,
                            1.0,
                        )
                        .into_iter()
                        .map(Scene::Window),
                );
            }
        }
        for (surface, at) in &plan.lower {
            elements.extend(
                layer(renderer, surface, *at)
                    .into_iter()
                    .map(Scene::Surface),
            );
        }

        let size = plan.size;
        let buffer: Size<i32, BufferCoord> = (size.w, size.h).into();
        let mut target: T = renderer
            .create_buffer(Fourcc::Abgr8888, buffer)
            .map_err(|error| failed(&error))?;
        let mut framebuffer = renderer.bind(&mut target).map_err(|error| failed(&error))?;
        OutputDamageTracker::new(size, plan.scale, Transform::Normal)
            .render_output(renderer, &mut framebuffer, 0, &elements, BACKDROP)
            .map_err(|error| failed(&format!("{error:?}")))?;
        let mapping = renderer
            .copy_framebuffer(&framebuffer, Rectangle::from_size(buffer), Fourcc::Abgr8888)
            .map_err(|error| failed(&error))?;
        // Rows come back top first from both renderers. GLES's mapping says
        // `flipped()` unconditionally, but the damage tracker drawing into an
        // offscreen texture has already put the image the right way up, and
        // turning it over again gave upside-down pictures on a seat (found on
        // hardware; pixman headless says false and was always right).
        let bytes = renderer
            .map_texture(&mapping)
            .map_err(|error| failed(&error))?;

        let (width, height) = (
            u32::try_from(size.w).unwrap_or(0),
            u32::try_from(size.h).unwrap_or(0),
        );
        Ok(Pixels {
            width,
            height,
            rgba: bytes.to_vec(),
        })
    }
}
