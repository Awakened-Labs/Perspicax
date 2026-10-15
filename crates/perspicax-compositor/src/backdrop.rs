//! What an output shows where nothing else is: a colour, the *ground*, and
//! on it at most one image, the logo.
//!
//! | | Ground | Image |
//! |---|---|---|
//! | Locked | the theme's `lock` | the logo, above swaylock's ring |
//! | The splash still up on the output | the theme's `splash` | the logo, centred |
//! | Otherwise | [`BACKDROP`](crate::BACKDROP) grey | none |
//!
//! While locked, a seat draws nothing but the lock surfaces, and swaylock is
//! dressed with no colour of its own wherever perspicax draws beneath it
//! (`perspicax_config`'s `swaylock`). Its surface is clear but for its ring
//! and its words, and the ground and the logo show through. They are on the
//! screen from the moment the session locks, before the lock client has
//! drawn anything. A lock client that paints its whole surface hides them,
//! as it should: the screen is then its to draw.
//!
//! # The splash
//!
//! A session starts on a seat with the splash, from its first frame, instead
//! of a dark screen while the desktop comes up behind it. It is beneath
//! everything, never a curtain over the windows: what a window shows, the
//! person sees, and what the facts say they see stays true. The shell's
//! wallpaper covers it the moment it draws, and so ends it on that output:
//! a surface in the background layer, covering the whole output, committing
//! a buffer ([`Compositor::layer_committed`]). Where nothing ever covers it,
//! with no wallpaper or no shell, it ends everywhere after a while. It never
//! comes back in that session, and a save turning `[splash]` off ends it at
//! once. Locked meanwhile, the lock is shown; unlocked, the splash again, if
//! it is still up there.
//!
//! # The art
//!
//! The logo is Perspicax's lockup, the mark over its name, unless the theme
//! names a PNG of the person's, or none. The lockup is lettered two ways:
//! navy for a light ground and white for a dark one, chosen by the rule every
//! ink is chosen by, [`Colour::ink`]. The two PNGs in `art/` are drawn from
//! the brand's SVG masters, which share one framing and hold no text, so
//! drawing them again gives the same bytes. From the repository's root, with
//! the brand's assets beside it:
//!
//! ```text
//! rsvg-convert -w 1024 -o crates/perspicax-compositor/art/perspicax-lockup.png \
//!     ../assets/brand/perspicax-lockup.svg
//! rsvg-convert -w 1024 -o crates/perspicax-compositor/art/perspicax-lockup-reversed.png \
//!     ../assets/brand/perspicax-lockup-reversed.svg
//! ```
//!
//! That is 1024 by 809 pixels: a quarter of the height of a monitor up to
//! 3236 pixels high. A taller one shows it at its own size, never enlarged.
//!
//! # Never on the way to locking
//!
//! Decoding is done on threads, so neither a slow disk nor a hung home folder
//! can stall the compositor: the built-in pair once, as the seat opens, and a
//! person's file each time the config is read. What they decode arrives over
//! a channel, and until it does the ground is shown alone.
//!
//! The image each output shows is built ahead, too: when the output is lit or
//! rescaled, when the theme changes and when art arrives. So locking does no
//! pixel work. It is built in the monitor's own pixels, shrunk to exactly the
//! size it covers and drawn one pixel to one, so a fractional scale neither
//! crops it nor smears it. A buffer is kept for as long as what it was built
//! from is unchanged, and its unchanged id is what lets the damage tracker
//! leave it alone.

use std::{
    io::{BufRead, Seek},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use perspicax_policy::{Colour, Logo, Palette, Rgba, Role};
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Physical, Point, Rectangle, Size, Transform},
};

use crate::state::Compositor;

/// Perspicax's lockup, lettered navy, for a light ground.
const NAVY: &[u8] = include_bytes!("../art/perspicax-lockup.png");

/// Perspicax's lockup, lettered white, for a dark ground.
const WHITE: &[u8] = include_bytes!("../art/perspicax-lockup-reversed.png");

/// The widest and the tallest logo of the person's that is shown, in pixels.
/// A larger one is refused from its header, before its pixels are read.
const LARGEST: u32 = 4096;

/// What the PNG decoder may allocate: room for the largest logo at sixteen
/// bits a channel, before they are cut to eight.
const DECODER_BYTES: usize = 128 << 20;

/// Half the height of swaylock's indicator as it draws it by default, in
/// logical pixels: a ring of radius 50, 7 thick, centred on the screen.
const RING: i64 = 57;

/// The room between the lock's logo and the top of that ring.
const ABOVE_RING: i64 = 40;

/// A decoded image: premultiplied ARGB8888, which in memory is blue, green,
/// red, then alpha, as smithay's [`Fourcc::Argb8888`] buffers are.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Picture {
    size: (u32, u32),
    pixels: Vec<u8>,
}

/// Which of the built-in lockups is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lettering {
    Navy,
    White,
}

/// The lettering that reads on `ground`: white where white ink would be
/// written on it, navy where black would.
fn lettering(ground: Rgba) -> Lettering {
    if ground.colour().ink() == Colour::rgb(0xff, 0xff, 0xff) {
        Lettering::White
    } else {
        Lettering::Navy
    }
}

/// The colour an output is cleared to under `ground`: opaque, since nothing
/// is behind it.
fn clear(ground: Rgba) -> [f32; 4] {
    let channel = |value: u8| f32::from(value) / 255.0;
    [channel(ground.r), channel(ground.g), channel(ground.b), 1.0]
}

/// Where on an output an image goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// Centred across, its foot [`ABOVE_RING`] above swaylock's ring, and
    /// never in the top tenth of the screen.
    Lock,
    /// Centred.
    Splash,
}

/// An image's place on one output: its size in the output's logical
/// pixels, and its size and top-left corner in the monitor's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placed {
    logical: Size<i32, Logical>,
    physical: Size<i32, Physical>,
    at: Point<i32, Physical>,
}

/// Where an image of `art` pixels goes on a monitor of `mode` pixels at
/// `scale`, as `placement` puts it: as large as fits in two fifths of the
/// output's width and a quarter of its height with its shape kept, and never
/// more than one of its pixels to one of the monitor's. `None` where it would
/// be nothing at all.
fn place(
    mode: Size<i32, Physical>,
    scale: f64,
    art: (u32, u32),
    placement: Placement,
) -> Option<Placed> {
    let (art_w, art_h) = (i64::from(art.0), i64::from(art.1));
    if art_w == 0 || art_h == 0 || scale <= 0.0 {
        return None;
    }
    // The output's logical size, rounded up as `Space::output_geometry`
    // rounds it.
    let logical = mode.to_f64().to_logical(scale).to_i32_ceil::<i32>();
    let (width, height) = (i64::from(logical.w), i64::from(logical.h));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a side of a picture at most LARGEST wide, over a scale of at least 1/4"
    )]
    let own = |side: i64| (side as f64 / scale).floor() as i64;
    let most_w = (width * 2 / 5).min(own(art_w));
    let mut most_h = (height / 4).min(own(art_h));
    let foot = height / 2 - RING - ABOVE_RING;
    if placement == Placement::Lock {
        most_h = most_h.min(foot - (height + 9) / 10);
    }
    // Bound by whichever side runs out first; the other follows the art.
    let (w, h) = if most_w * art_h <= most_h * art_w {
        (most_w, most_w * art_h / art_w)
    } else {
        (most_h * art_w / art_h, most_h)
    };
    if w < 1 || h < 1 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a logical length no larger than the monitor, times its scale"
    )]
    let physical = |side: i64| (side as f64 * scale).round() as i32;
    let size = Size::from((physical(w), physical(h)));
    let down = match placement {
        Placement::Lock => physical(foot) - size.h,
        Placement::Splash => (mode.h - size.h) / 2,
    };
    Some(Placed {
        logical: Size::from((i32::try_from(w).ok()?, i32::try_from(h).ok()?)),
        physical: size,
        at: Point::from(((mode.w - size.w) / 2, down)),
    })
}

/// `pixels`, four bytes each and `from` in size, shrunk to `to`: each pixel
/// the mean of the ones it covers, weighed by how much of each it covers,
/// which is exact for any shrinking. Premultiplied pixels stay so. `None`
/// for a size that is not smaller or the same, or pixels not `from` in size.
fn shrink(pixels: &[u8], from: (u32, u32), to: (u32, u32)) -> Option<Vec<u8>> {
    let side = |length: u32| usize::try_from(length).ok();
    let (from_w, from_h, to_w, to_h) = (side(from.0)?, side(from.1)?, side(to.0)?, side(to.1)?);
    if to_w == 0 || to_h == 0 || to_w > from_w || to_h > from_h {
        return None;
    }
    if pixels.len() != from_w * from_h * 4 {
        return None;
    }
    let (columns, rows) = (spans(from_w, to_w), spans(from_h, to_h));
    let whole = u64::try_from(from_w * from_h).ok()?;
    let mut shrunk = Vec::with_capacity(to_w * to_h * 4);
    // One row of sums at a time: the rows each row of `to` covers, weighed,
    // and then each of its columns.
    let mut line = vec![0_u32; from_w * 4];
    for row in &rows {
        line.fill(0);
        for &(y, weight) in row {
            let source = &pixels[y * from_w * 4..(y + 1) * from_w * 4];
            for (sum, &value) in line.iter_mut().zip(source) {
                *sum += u32::from(value) * weight;
            }
        }
        for column in &columns {
            let mut sums = [0_u64; 4];
            for &(x, weight) in column {
                for (sum, &value) in sums.iter_mut().zip(&line[x * 4..x * 4 + 4]) {
                    *sum += u64::from(value) * u64::from(weight);
                }
            }
            shrunk.extend(sums.map(|sum| u8::try_from((sum + whole / 2) / whole).unwrap_or(0xff)));
        }
    }
    Some(shrunk)
}

/// For each of `to` cells along a side `from` cells long, the cells it
/// covers and how much of each: a `from` cell is `to` units long and a `to`
/// cell is `from`, so every overlap is a whole number of units and each `to`
/// cell's add up to `from`.
fn spans(from: usize, to: usize) -> Vec<Vec<(usize, u32)>> {
    (0..to)
        .map(|cell| {
            let (start, end) = (cell * from, (cell + 1) * from);
            (start / to..end.div_ceil(to))
                .map(|source| {
                    let overlap = end.min((source + 1) * to) - start.max(source * to);
                    (source, u32::try_from(overlap).unwrap_or(u32::MAX))
                })
                .collect()
        })
        .collect()
}

/// Why a logo could not be read.
#[derive(Debug, thiserror::Error)]
enum Unread {
    #[error("{0}")]
    Open(#[from] std::io::Error),
    #[error("not a PNG it can read: {0}")]
    Png(#[from] png::DecodingError),
    #[error("{0} by {1} pixels, more than {LARGEST} a side")]
    Large(u32, u32),
}

/// A PNG as premultiplied ARGB8888. Its size is read first, from its header,
/// and one over [`LARGEST`] a side is refused before its pixels are read.
fn decode(source: impl BufRead + Seek) -> Result<Picture, Unread> {
    let mut decoder = png::Decoder::new_with_limits(
        source,
        png::Limits {
            bytes: DECODER_BYTES,
        },
    );
    let (width, height) = decoder.read_header_info()?.size();
    if width > LARGEST || height > LARGEST {
        return Err(Unread::Large(width, height));
    }
    // Palettes and low depths expanded and sixteen bits cut to eight, so
    // what comes out is grey or colour, with or without alpha, a byte each.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut read = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or(Unread::Large(width, height))?
    ];
    let frame = reader.next_frame(&mut read)?;
    read.truncate(frame.buffer_size());
    let pixels = read
        .chunks_exact(frame.color_type.samples())
        .flat_map(|pixel| {
            let (r, g, b, a) = match *pixel {
                [r, g, b, a] => (r, g, b, a),
                [r, g, b] => (r, g, b, 0xff),
                [grey, a] => (grey, grey, grey, a),
                [grey] => (grey, grey, grey, 0xff),
                _ => (0, 0, 0, 0),
            };
            let times = |channel: u8| {
                u8::try_from((u16::from(channel) * u16::from(a) + 127) / 255).unwrap_or(0xff)
            };
            [times(b), times(g), times(r), a]
        })
        .collect();
    Ok(Picture {
        size: (frame.width, frame.height),
        pixels,
    })
}

/// Art decoded off the compositor's thread, on its way to the backdrop.
#[derive(Debug)]
#[cfg_attr(
    not(any(feature = "seat", test)),
    expect(dead_code, reason = "a person's logo is read on a seat alone")
)]
pub(crate) enum Arrived {
    /// Perspicax's lockup: lettered navy, then white.
    Builtin(Box<(Picture, Picture)>),
    /// The person's logo, as read for the asking numbered `asked`.
    File {
        asked: u64,
        path: PathBuf,
        read: Result<Picture, String>,
    },
}

/// A reading of the person's logo to start: `read` starts it, on a seat.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    number: u64,
    path: PathBuf,
}

/// Perspicax's lockup, decoded.
fn builtin() -> Result<Arrived, Unread> {
    let navy = decode(std::io::Cursor::new(NAVY))?;
    let white = decode(std::io::Cursor::new(WHITE))?;
    Ok(Arrived::Builtin(Box::new((navy, white))))
}

/// Decode Perspicax's lockup on a thread of its own, and send it to `to`.
#[cfg(feature = "seat")]
pub(crate) fn decode_builtin(to: smithay::reexports::calloop::channel::Sender<Arrived>) {
    spawn("perspicax-lockup", move || match builtin() {
        // Unsent only once the session has ended, with nobody to show it.
        Ok(arrived) => {
            let _ = to.send(arrived);
        }
        Err(error) => tracing::error!(
            %error,
            "Perspicax's lockup could not be decoded; the backdrop goes without it"
        ),
    });
}

/// Read and decode the person's logo on a thread of its own, and send what
/// came of it to `to`.
#[cfg(feature = "seat")]
pub(crate) fn read(asked: Asked, to: smithay::reexports::calloop::channel::Sender<Arrived>) {
    spawn("perspicax-logo", move || {
        let read = std::fs::File::open(&asked.path)
            .map_err(Unread::from)
            .and_then(|file| decode(std::io::BufReader::new(file)))
            .map_err(|error| error.to_string());
        let arrived = Arrived::File {
            asked: asked.number,
            path: asked.path,
            read,
        };
        let _ = to.send(arrived);
    });
}

/// Run `work` on a thread named `name`. Without one the backdrop goes
/// without what it would have decoded.
#[cfg(feature = "seat")]
fn spawn(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(error) = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
    {
        tracing::warn!(%error, thread = name, "could not start a thread to decode art");
    }
}

/// What the config says the backdrop shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Look {
    /// The lock screen's ground: the theme's `lock`.
    pub(crate) lock: Rgba,
    /// The splash's ground: the theme's `splash`.
    pub(crate) splash: Rgba,
    /// The logo, with a file's path already found.
    pub(crate) logo: Logo,
}

impl Look {
    /// How `palette` has it look, with Perspicax's lockup.
    pub(crate) fn of(palette: &Palette) -> Self {
        Self {
            lock: palette[Role::Lock],
            splash: palette[Role::Splash],
            logo: Logo::Perspicax,
        }
    }
}

impl Default for Look {
    fn default() -> Self {
        Self::of(&Palette::default())
    }
}

/// The art decoded so far.
#[derive(Debug, Default)]
struct Art {
    /// Perspicax's lockup, lettered navy, then white.
    builtin: Option<(Rc<Picture>, Rc<Picture>)>,
    /// The person's logo as last read: from where, and what came of it,
    /// `None` for a file that could not be read.
    file: Option<(PathBuf, Option<Rc<Picture>>)>,
}

impl Art {
    /// What `logo` shows on `ground`, once it is here.
    fn on(&self, logo: &Logo, ground: Rgba) -> Option<&Rc<Picture>> {
        let builtin = || {
            self.builtin
                .as_ref()
                .map(|(navy, white)| match lettering(ground) {
                    Lettering::Navy => navy,
                    Lettering::White => white,
                })
        };
        match logo {
            Logo::None => None,
            Logo::Perspicax => builtin(),
            Logo::File(path) => match &self.file {
                Some((read, Some(picture))) if read == path => Some(picture),
                // Said once, as it arrived: Perspicax's in its place.
                Some((read, None)) if read == path => builtin(),
                // Not read yet.
                _ => None,
            },
        }
    }
}

/// One lit monitor, and the images built for it.
#[derive(Debug)]
struct Screen {
    output: Output,
    mode: Size<i32, Physical>,
    scale: f64,
    /// The lock's, built whether or not the session is locked, so that
    /// locking does no pixel work.
    lock: Option<Built>,
    /// The splash's, while the splash is up on this output.
    splash: Option<Built>,
}

/// An image built for one output, and what it was built from.
#[derive(Debug)]
struct Built {
    picture: Rc<Picture>,
    placed: Placed,
    buffer: MemoryRenderBuffer,
}

impl Built {
    /// `picture`, shrunk to the pixels `placed` covers.
    fn new(picture: Rc<Picture>, placed: Placed) -> Option<Self> {
        let to = (
            u32::try_from(placed.physical.w).ok()?,
            u32::try_from(placed.physical.h).ok()?,
        );
        let pixels = shrink(&picture.pixels, picture.size, to)?;
        let buffer = MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (placed.physical.w, placed.physical.h),
            1,
            Transform::Normal,
            None,
        );
        Some(Self {
            picture,
            placed,
            buffer,
        })
    }

    /// Make `slot` hold `wanted`: kept as it is if it already does, so its
    /// buffer keeps its id, and built afresh if not. Whether it changed.
    fn keep_or_build(slot: &mut Option<Self>, wanted: Option<(&Rc<Picture>, Placed)>) -> bool {
        let unchanged = match (&*slot, &wanted) {
            (Some(built), Some((picture, placed))) => {
                Rc::ptr_eq(&built.picture, picture) && built.placed == *placed
            }
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            *slot = wanted.and_then(|(picture, placed)| Self::new(picture.clone(), placed));
        }
        !unchanged
    }

    fn image(&self) -> Image {
        Image {
            buffer: self.buffer.clone(),
            placed: self.placed,
        }
    }
}

/// The splash, while it is up: the outputs it has ended on, by name, so
/// that a monitor plugged in again does not bring it back.
#[derive(Debug, Default)]
struct Splash {
    done: Vec<String>,
}

/// What each output shows where nothing else is. See the module.
#[derive(Debug, Default)]
pub(crate) struct Backdrop {
    look: Look,
    art: Art,
    /// The newest reading of the person's logo asked for. An answer to an
    /// older one is stale, and dropped.
    asked: u64,
    screens: Vec<Screen>,
    splash: Option<Splash>,
}

/// The image an output shows on its ground, ready to be drawn: its buffer
/// shared, with the same id, so it costs nothing to hand out.
#[derive(Debug, Clone)]
pub(crate) struct Image {
    buffer: MemoryRenderBuffer,
    placed: Placed,
}

impl Image {
    /// The image as an element to draw: its buffer's pixels one to one with
    /// the monitor's, the whole buffer shown over its logical size.
    pub(crate) fn element<R>(&self, renderer: &mut R) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Placed {
            logical,
            physical,
            at,
        } = self.placed;
        let src = Rectangle::from_size(Size::<f64, Logical>::from((
            f64::from(physical.w),
            f64::from(physical.h),
        )));
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            at.to_f64(),
            &self.buffer,
            None,
            Some(src),
            Some(logical),
            Kind::Unspecified,
        )
        .inspect_err(|error| tracing::warn!(?error, "the backdrop's image was not drawn"))
        .ok()
    }
}

impl Backdrop {
    /// Show what `look` says. Returns the reading to start when the logo is
    /// a file: it is read afresh each time the config is, so a logo changed
    /// on disk, or one that could not be read, is read again at the next
    /// save. What was read before goes on being shown until it arrives.
    pub(crate) fn dress(&mut self, look: Look) -> Option<Asked> {
        let asked = match &look.logo {
            Logo::File(path) => {
                self.asked += 1;
                Some(Asked {
                    number: self.asked,
                    path: path.clone(),
                })
            }
            Logo::Perspicax | Logo::None => None,
        };
        self.look = look;
        self.build();
        asked
    }

    /// Take art a thread decoded. Whether anything shown changed, so that
    /// the outputs must be drawn again.
    pub(crate) fn arrived(&mut self, arrived: Arrived) -> bool {
        match arrived {
            Arrived::Builtin(pair) => {
                let (navy, white) = *pair;
                self.art.builtin = Some((Rc::new(navy), Rc::new(white)));
            }
            Arrived::File { asked, .. } if asked != self.asked => return false,
            Arrived::File { path, read, .. } => {
                let picture = read
                    .inspect_err(|error| {
                        tracing::warn!(
                            logo = %path.display(),
                            %error,
                            "the logo cannot be shown; Perspicax's is, until the config is saved again"
                        );
                    })
                    .ok();
                // The same picture read again keeps its buffers, and the
                // damage tracker has nothing to redraw.
                if let Some((was, Some(old))) = &self.art.file
                    && *was == path
                    && picture.as_ref() == Some(&**old)
                {
                    return false;
                }
                self.art.file = Some((path, picture.map(Rc::new)));
            }
        }
        self.build()
    }

    /// The monitors lit now: each output, its mode in pixels, and its scale.
    /// One no longer lit takes its buffers with it.
    pub(crate) fn screens(
        &mut self,
        lit: impl IntoIterator<Item = (Output, Size<i32, Physical>, f64)>,
    ) {
        let mut before = std::mem::take(&mut self.screens);
        self.screens = lit
            .into_iter()
            .map(|(output, mode, scale)| {
                let was = before.iter_mut().find(|screen| screen.output == output);
                let (lock, splash) =
                    was.map_or((None, None), |was| (was.lock.take(), was.splash.take()));
                Screen {
                    output,
                    mode,
                    scale,
                    lock,
                    splash,
                }
            })
            .collect();
        self.build();
    }

    /// Put the splash up on every output, until [`Self::splash_done_on`]
    /// or [`Self::end_splash`].
    fn begin_splash(&mut self) {
        self.splash = Some(Splash::default());
        self.build();
    }

    /// Whether the splash is still up on `output`.
    pub(crate) fn splash_on(&self, output: &Output) -> bool {
        self.splash
            .as_ref()
            .is_some_and(|splash| !splash.done.iter().any(|name| *name == output.name()))
    }

    /// A wallpaper covers `output`: the splash is done there. Whether it was
    /// still up there.
    pub(crate) fn splash_done_on(&mut self, output: &Output) -> bool {
        if !self.splash_on(output) {
            return false;
        }
        if let Some(splash) = &mut self.splash {
            splash.done.push(output.name());
        }
        self.build();
        true
    }

    /// End the splash everywhere, for the rest of the session. Whether any
    /// output was still showing it.
    pub(crate) fn end_splash(&mut self) -> bool {
        let shown = self
            .screens
            .iter()
            .any(|screen| self.splash_on(&screen.output));
        self.splash = None;
        self.build();
        shown
    }

    /// What `output` shows where nothing else is, `locked` or not: the colour
    /// to clear it to, and the image on it, if any.
    pub(crate) fn shown(&self, output: &Output, locked: bool) -> ([f32; 4], Option<Image>) {
        let screen = self.screens.iter().find(|screen| &screen.output == output);
        if locked {
            let image = screen.and_then(|screen| screen.lock.as_ref());
            (clear(self.look.lock), image.map(Built::image))
        } else if self.splash_on(output) {
            let image = screen.and_then(|screen| screen.splash.as_ref());
            (clear(self.look.splash), image.map(Built::image))
        } else {
            (crate::BACKDROP, None)
        }
    }

    /// Build each image wanted and not built yet, or built from something
    /// that has changed since, and drop each no longer wanted. Whether any
    /// changed.
    fn build(&mut self) -> bool {
        let mut changed = false;
        for screen in &mut self.screens {
            let splash_up = self.splash.as_ref().is_some_and(|splash| {
                !splash.done.iter().any(|name| *name == screen.output.name())
            });
            let wanted = |ground: Rgba, placement: Placement| {
                self.art.on(&self.look.logo, ground).and_then(|picture| {
                    place(screen.mode, screen.scale, picture.size, placement)
                        .map(|placed| (picture, placed))
                })
            };
            let lock = wanted(self.look.lock, Placement::Lock);
            let splash = wanted(self.look.splash, Placement::Splash).filter(|_| splash_up);
            changed |= Built::keep_or_build(&mut screen.lock, lock);
            changed |= Built::keep_or_build(&mut screen.splash, splash);
        }
        changed
    }
}

impl Compositor {
    /// Put the splash up until a wallpaper covers each output, and take it
    /// down everywhere after `lasts` at the most, should nothing ever cover
    /// it: no wallpaper, or no shell.
    pub(crate) fn show_splash(&mut self, lasts: Duration) {
        self.backdrop.begin_splash();
        let armed = self
            .loop_handle
            .insert_source(Timer::from_duration(lasts), |_, (), state| {
                if state.backdrop.end_splash() {
                    tracing::info!("nothing covered the splash in time; it is over");
                    state.backend.redraw();
                }
                TimeoutAction::Drop
            });
        if let Err(error) = armed {
            tracing::warn!(%error, "the splash ends only where a wallpaper covers it");
        }
        self.backend.redraw();
    }

    /// The splash as a test asks for it: in `palette`'s colours, with
    /// Perspicax's lockup decoded here and now rather than on a thread, as a
    /// seat would, for `lasts` at the most.
    pub(crate) fn splash_for_test(&mut self, lasts: Duration, palette: &Palette) {
        // Perspicax's lockup, so no reading is ever asked for.
        let _ = self.backdrop.dress(Look::of(palette));
        match builtin() {
            Ok(arrived) => {
                self.backdrop.arrived(arrived);
            }
            Err(error) => tracing::error!(%error, "Perspicax's lockup could not be decoded"),
        }
        self.show_splash(lasts);
    }
}

#[cfg(test)]
mod tests {
    use perspicax_policy::Builtin;
    use smithay::output::{PhysicalProperties, Subpixel};

    use super::*;

    const ART: (u32, u32) = (1024, 809);

    fn lock(mode: (i32, i32), scale: f64, art: (u32, u32)) -> Placed {
        place(mode.into(), scale, art, Placement::Lock).expect("placed")
    }

    #[test]
    fn the_lettering_is_whichever_reads_on_the_ground() {
        let lock = |builtin: Builtin| lettering(builtin.palette()[Role::Lock]);
        assert_eq!(lock(Builtin::Perspicax), Lettering::White, "on navy");
        assert_eq!(lock(Builtin::BreezeDark), Lettering::White);
        assert_eq!(
            lock(Builtin::BreezeLight),
            Lettering::Navy,
            "on pale blue-grey"
        );
        assert_eq!(
            lettering(Rgba::new(0xff, 0xff, 0x00, 0xff)),
            Lettering::Navy
        );
        assert_eq!(
            lettering(Rgba::new(0x80, 0x00, 0x00, 0xff)),
            Lettering::White
        );
    }

    #[test]
    fn the_ground_is_cleared_opaque_whatever_its_colour_says() {
        assert_eq!(
            clear(Rgba::new(0xff, 0x00, 0x33, 0x40)),
            [1.0, 0.0, 0.2, 1.0]
        );
    }

    #[test]
    fn the_lock_logo_fits_a_quarter_of_the_height_centred_above_the_ring() {
        let placed = lock((1920, 1080), 1.0, ART);
        assert_eq!(placed.logical, (341, 270).into(), "a quarter of 1080 tall");
        assert_eq!(placed.physical, placed.logical.to_physical(1));
        assert_eq!(
            placed.at,
            (789, 443 - 270).into(),
            "its foot 97 above the middle"
        );

        let laptop = lock((1366, 768), 1.0, ART);
        assert_eq!(laptop.logical, (243, 192).into());
        assert_eq!(laptop.at, (561, 287 - 192).into());

        let portrait = lock((1080, 1920), 1.0, ART);
        assert_eq!(
            portrait.logical,
            (432, 341).into(),
            "two fifths of the width, where that runs out first"
        );
        assert_eq!(portrait.at, (324, 863 - 341).into());
    }

    #[test]
    fn at_any_scale_the_logo_is_whole_logical_pixels_and_exact_physical_ones() {
        let at_one = lock((3840, 2160), 1.0, ART);
        assert_eq!(at_one.logical, (683, 540).into());
        assert_eq!(at_one.physical, (683, 540).into());

        let at_half = lock((3840, 2160), 1.5, ART);
        assert_eq!(at_half.logical, (455, 360).into(), "of 2560 by 1440");
        assert_eq!(
            at_half.physical,
            (683, 540).into(),
            "each logical length times 1.5, rounded"
        );
        assert_eq!(
            at_half.at.x,
            (3840 - 683) / 2,
            "centred in the monitor's pixels"
        );

        let at_two = lock((3840, 2160), 2.0, ART);
        assert_eq!(at_two.logical, (341, 270).into(), "as 1920 by 1080");
        assert_eq!(at_two.physical, (682, 540).into());
    }

    #[test]
    fn the_lock_logo_never_reaches_the_ring_nor_the_top_tenth() {
        for mode in [
            (1920, 1080),
            (1366, 768),
            (1080, 1920),
            (3840, 2160),
            (1280, 400),
        ] {
            for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
                let Some(placed) = place(mode.into(), scale, ART, Placement::Lock) else {
                    continue;
                };
                let height = (f64::from(mode.1) / scale).ceil();
                let ring = (height / 2.0 - 57.0) * scale;
                let foot = f64::from(placed.at.y + placed.physical.h);
                assert!(
                    ring - foot >= 40.0 * scale - 1.0,
                    "{mode:?} at {scale}: its foot at {foot}, the ring at {ring}"
                );
                assert!(
                    f64::from(placed.at.y) >= height / 10.0 * scale - 1.0,
                    "{mode:?} at {scale}: its top at {}",
                    placed.at.y
                );
            }
        }
        assert_eq!(
            place((640, 200).into(), 1.0, ART, Placement::Lock),
            None,
            "no room above the ring at all"
        );
    }

    #[test]
    fn the_splash_logo_is_as_large_as_the_locks_and_centred() {
        let splash = |mode: (i32, i32), scale| {
            place(mode.into(), scale, ART, Placement::Splash).expect("placed")
        };
        let placed = splash((1920, 1080), 1.0);
        assert_eq!(placed.logical, lock((1920, 1080), 1.0, ART).logical);
        assert_eq!(placed.at, ((1920 - 341) / 2, (1080 - 270) / 2).into());
        let portrait = splash((1080, 1920), 1.0);
        assert_eq!(portrait.at, ((1080 - 432) / 2, (1920 - 341) / 2).into());
        let at_half = splash((3840, 2160), 1.5);
        assert_eq!(at_half.physical, (683, 540).into());
        assert_eq!(at_half.at, ((3840 - 683) / 2, (2160 - 540) / 2).into());
        assert!(
            place((640, 200).into(), 1.0, ART, Placement::Splash).is_some(),
            "with no ring to keep clear of, it has room where the lock has none"
        );
    }

    #[test]
    fn the_logo_is_never_enlarged() {
        let huge = lock((7680, 4320), 1.0, ART);
        assert_eq!(huge.physical, (1024, 809).into(), "at its own size");
        let small = lock((1920, 1080), 1.0, (100, 50));
        assert_eq!(small.physical, (100, 50).into());
        for mode in [(1920, 1080), (3840, 2160), (7680, 4320), (5120, 1440)] {
            for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.5] {
                for art in [ART, (101, 51), (33, 97), (4096, 7), (3, 3)] {
                    for placement in [Placement::Lock, Placement::Splash] {
                        let Some(placed) = place(mode.into(), scale, art, placement) else {
                            continue;
                        };
                        let physical = placed.physical;
                        assert!(
                            physical.w <= art.0 as i32 && physical.h <= art.1 as i32,
                            "{art:?} on {mode:?} at {scale}, {placement:?}: {physical:?}"
                        );
                    }
                }
            }
        }
    }

    /// A picture of one row, each pixel a grey `value` as opaque as `alpha`,
    /// premultiplied.
    fn row(pixels: &[(u8, u8)]) -> Vec<u8> {
        pixels
            .iter()
            .flat_map(|&(value, alpha)| [value, value, value, alpha])
            .collect()
    }

    #[test]
    fn a_two_by_two_shrinks_to_the_mean_of_its_four() {
        let pixels = [
            [0, 10, 20, 255],
            [40, 50, 60, 255],
            [80, 90, 100, 255],
            [1, 2, 3, 255],
        ];
        let shrunk = shrink(pixels.as_flattened(), (2, 2), (1, 1)).expect("shrunk");
        assert_eq!(shrunk, [30, 38, 46, 255], "rounded to the nearest");
    }

    #[test]
    fn a_solid_picture_stays_solid() {
        let solid: Vec<u8> = [12, 34, 56, 255].repeat(7 * 5);
        let shrunk = shrink(&solid, (7, 5), (3, 2)).expect("shrunk");
        assert_eq!(shrunk, [12, 34, 56, 255].repeat(3 * 2));
        assert_eq!(
            shrink(&solid, (7, 5), (7, 5)),
            Some(solid),
            "the same size, as it was"
        );
    }

    #[test]
    fn a_pixel_split_between_two_is_shared_by_both() {
        let shrunk = shrink(&row(&[(0, 255), (255, 255), (0, 255)]), (3, 1), (2, 1));
        assert_eq!(
            shrunk,
            Some(row(&[(85, 255), (85, 255)])),
            "two thirds and a third"
        );
    }

    #[test]
    fn alpha_stays_premultiplied() {
        let shrunk = shrink(&row(&[(255, 255), (0, 0)]), (2, 1), (1, 1));
        assert_eq!(
            shrunk,
            Some(row(&[(128, 128)])),
            "white half covered is half white, not white half seen"
        );
    }

    #[test]
    fn shrink_refuses_to_enlarge() {
        let one = row(&[(255, 255)]);
        assert_eq!(shrink(&one, (1, 1), (2, 1)), None);
        assert_eq!(shrink(&one, (1, 1), (1, 2)), None);
        assert_eq!(shrink(&one, (1, 1), (0, 1)), None, "nor to nothing");
        assert_eq!(
            shrink(&one, (2, 1), (1, 1)),
            None,
            "nor pixels of another size"
        );
    }

    #[test]
    fn the_built_in_lockups_decode_lettered_navy_and_white() {
        let navy = decode(std::io::Cursor::new(NAVY)).expect("navy");
        let white = decode(std::io::Cursor::new(WHITE)).expect("white");
        for picture in [&navy, &white] {
            assert_eq!(picture.size, ART);
            assert!(
                picture
                    .pixels
                    .chunks_exact(4)
                    .all(|pixel| pixel[..3].iter().all(|&channel| channel <= pixel[3])),
                "premultiplied"
            );
            assert!(
                picture
                    .pixels
                    .chunks_exact(4)
                    .any(|pixel| pixel == [0x3c, 0x6a, 0xc0, 0xff]),
                "the mark in terracotta, blue first"
            );
        }
        // The name, in the bottom fifth: dark in one and white in the other.
        let lettered = |picture: &Picture, wanted: fn(&[u8]) -> bool| {
            let foot = (ART.1 * 4 / 5 * ART.0 * 4) as usize;
            picture.pixels[foot..]
                .chunks_exact(4)
                .filter(|pixel| pixel[3] == 0xff && wanted(pixel))
                .count()
        };
        let dark = |pixel: &[u8]| pixel[..3].iter().all(|&channel| channel < 0x50);
        let light = |pixel: &[u8]| pixel[..3].iter().all(|&channel| channel == 0xff);
        assert!(lettered(&navy, dark) > 10_000, "navy lettering");
        assert_eq!(lettered(&navy, light), 0);
        assert!(lettered(&white, light) > 10_000, "white lettering");
        assert_eq!(lettered(&white, dark), 0);
    }

    /// `pixels` of `colour`, written as a PNG `size` in pixels, at eight bits
    /// a channel.
    fn png(size: (u32, u32), colour: png::ColorType, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, size.0, size.1);
        encoder.set_color(colour);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header");
        writer.write_image_data(pixels).expect("written");
        writer.finish().expect("finished");
        bytes
    }

    #[test]
    fn a_logo_is_read_premultiplied_whatever_its_colour_type() {
        let rgba = png(
            (2, 1),
            png::ColorType::Rgba,
            &[0xff, 0, 0, 0xff, 0xff, 0xff, 0xff, 0x80],
        );
        let picture = decode(std::io::Cursor::new(rgba)).expect("decoded");
        assert_eq!(picture.size, (2, 1));
        assert_eq!(
            picture.pixels,
            [0, 0, 0xff, 0xff, 0x80, 0x80, 0x80, 0x80],
            "opaque red, blue first, then half-seen white"
        );
        let grey = png((1, 1), png::ColorType::Grayscale, &[0x40]);
        let picture = decode(std::io::Cursor::new(grey)).expect("decoded");
        assert_eq!(picture.pixels, [0x40, 0x40, 0x40, 0xff]);
    }

    #[test]
    fn a_logo_too_large_or_not_a_png_is_refused() {
        let wide = png(
            (LARGEST + 1, 1),
            png::ColorType::Grayscale,
            &vec![0; LARGEST as usize + 1],
        );
        let refused = decode(std::io::Cursor::new(wide));
        assert!(
            matches!(refused, Err(Unread::Large(4097, 1))),
            "{refused:?}"
        );
        let gif = decode(std::io::Cursor::new(b"GIF89a".to_vec()));
        assert!(matches!(gif, Err(Unread::Png(_))), "{gif:?}");
    }

    fn output(name: &str) -> Output {
        Output::new(
            name.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".to_owned(),
                model: "test".to_owned(),
            },
        )
    }

    /// A small stand-in for a lockup, `tint` in every pixel.
    fn picture(tint: u8) -> Picture {
        Picture {
            size: (64, 32),
            pixels: [tint, tint, tint, 0xff].repeat(64 * 32),
        }
    }

    /// Stand-ins for Perspicax's lockup: lettered "navy", then "white".
    fn stand_ins() -> Arrived {
        Arrived::Builtin(Box::new((picture(0x10), picture(0xf0))))
    }

    /// The picture `output`'s lock image was built from, if one was.
    fn shown_picture<'a>(backdrop: &'a Backdrop, output: &Output) -> Option<&'a Rc<Picture>> {
        backdrop
            .screens
            .iter()
            .find(|screen| &screen.output == output)?
            .lock
            .as_ref()
            .map(|built| &built.picture)
    }

    #[test]
    fn locked_shows_the_lock_ground_and_once_it_is_here_the_logo() {
        let screen = output("DP-1");
        let mut backdrop = Backdrop::default();
        backdrop.screens([(screen.clone(), (1920, 1080).into(), 1.0)]);
        assert_eq!(backdrop.dress(Look::default()), None, "nothing to read");

        let (ground, image) = backdrop.shown(&screen, false);
        assert_eq!(ground, crate::BACKDROP, "unlocked, the grey");
        assert!(image.is_none());
        let (ground, image) = backdrop.shown(&screen, true);
        assert_eq!(ground, clear(Palette::default()[Role::Lock]));
        assert!(image.is_none(), "the ground alone, until the art is here");

        assert!(backdrop.arrived(stand_ins()), "drawn again");
        let (_, image) = backdrop.shown(&screen, true);
        let image = image.expect("the logo");
        assert_eq!(image.placed, lock((1920, 1080), 1.0, (64, 32)));
        assert!(
            backdrop.shown(&output("HDMI-1"), true).1.is_none(),
            "not lit"
        );
    }

    #[test]
    fn the_lettering_follows_the_lock_ground_and_none_shows_none() {
        let screen = output("DP-1");
        let mut backdrop = Backdrop::default();
        backdrop.screens([(screen.clone(), (1920, 1080).into(), 1.0)]);
        backdrop.arrived(stand_ins());
        let white = shown_picture(&backdrop, &screen).expect("built").clone();
        assert_eq!(white.pixels[0], 0xf0, "white lettering on navy");

        backdrop.dress(Look::of(&Builtin::BreezeLight.palette()));
        let navy = shown_picture(&backdrop, &screen).expect("built");
        assert_eq!(navy.pixels[0], 0x10, "navy on a light ground");

        backdrop.dress(Look {
            logo: Logo::None,
            ..Look::default()
        });
        assert!(shown_picture(&backdrop, &screen).is_none());
        assert_eq!(
            backdrop.shown(&screen, true).0,
            clear(Palette::default()[Role::Lock]),
            "the ground all the same"
        );
    }

    /// The id of a buffer, from its debug output: the one public word on
    /// it, and the whole of that output should it ever stop opening so.
    fn id(buffer: &MemoryRenderBuffer) -> String {
        let debug = format!("{buffer:?}");
        debug.split(", inner").next().unwrap_or_default().to_owned()
    }

    /// The id of the lock image's buffer on the only output. A buffer
    /// compared against is kept alive meanwhile: an id is free to be used
    /// again once its buffer is dropped.
    fn buffer(backdrop: &Backdrop) -> String {
        id(&backdrop.screens[0].lock.as_ref().expect("built").buffer)
    }

    #[test]
    fn a_buffer_is_kept_while_what_it_was_built_from_is_unchanged() {
        let screen = output("DP-1");
        let mut backdrop = Backdrop::default();
        backdrop.arrived(stand_ins());
        backdrop.screens([(screen.clone(), (1920, 1080).into(), 1.0)]);
        let kept = backdrop.screens[0]
            .lock
            .as_ref()
            .expect("built")
            .buffer
            .clone();
        let before = id(&kept);
        assert!(
            before.starts_with("MemoryRenderBuffer { id: Id("),
            "{before}"
        );
        backdrop.dress(Look::default());
        backdrop.screens([(screen.clone(), (1920, 1080).into(), 1.0)]);
        assert_eq!(buffer(&backdrop), before, "the same buffer, and its id");

        backdrop.screens([(screen.clone(), (1920, 1080).into(), 2.0)]);
        assert_ne!(buffer(&backdrop), before, "built again for its new scale");
        let placed = backdrop.screens[0].lock.as_ref().expect("built").placed;
        assert_eq!(
            placed.logical,
            (32, 16).into(),
            "half as many logical pixels"
        );
        assert_eq!(placed.physical, (64, 32).into(), "at its own pixels still");

        backdrop.screens([]);
        assert!(
            backdrop.screens.is_empty(),
            "an output gone takes its buffers"
        );
        drop(kept);
    }

    #[test]
    fn a_persons_logo_is_shown_once_read_and_a_stale_reading_is_dropped() {
        let screen = output("DP-1");
        let path = PathBuf::from("/home/me/logo.png");
        let mut backdrop = Backdrop::default();
        backdrop.screens([(screen.clone(), (1920, 1080).into(), 1.0)]);
        backdrop.arrived(stand_ins());
        let look = Look {
            logo: Logo::File(path.clone()),
            ..Look::default()
        };
        let first = backdrop.dress(look.clone()).expect("a reading");
        assert!(shown_picture(&backdrop, &screen).is_none(), "not read yet");
        let second = backdrop
            .dress(look.clone())
            .expect("read again at each save");
        assert_eq!(second.path, path);

        let answer = |asked: &Asked, tint| Arrived::File {
            asked: asked.number,
            path: asked.path.clone(),
            read: Ok(picture(tint)),
        };
        assert!(!backdrop.arrived(answer(&first, 0x01)), "stale");
        assert!(backdrop.arrived(answer(&second, 0x02)));
        assert_eq!(
            shown_picture(&backdrop, &screen).expect("shown").pixels[0],
            0x02
        );

        let third = backdrop.dress(look.clone()).expect("a reading");
        assert_eq!(
            shown_picture(&backdrop, &screen)
                .expect("still shown")
                .pixels[0],
            0x02,
            "what was read goes on being shown"
        );
        assert!(
            !backdrop.arrived(answer(&third, 0x02)),
            "the same picture again"
        );

        let fourth = backdrop.dress(look).expect("a reading");
        let unread = Arrived::File {
            asked: fourth.number,
            path: fourth.path,
            read: Err("no such file".to_owned()),
        };
        assert!(backdrop.arrived(unread));
        assert_eq!(
            shown_picture(&backdrop, &screen)
                .expect("Perspicax's")
                .pixels[0],
            0xf0,
            "one that cannot be read gives the lockup"
        );
    }

    #[test]
    fn the_splash_shows_until_a_wallpaper_covers_each_output_or_it_ends() {
        let (left, right) = (output("DP-1"), output("DP-2"));
        let mut backdrop = Backdrop::default();
        backdrop.screens([
            (left.clone(), (1920, 1080).into(), 1.0),
            (right.clone(), (1280, 1024).into(), 1.0),
        ]);
        backdrop.arrived(stand_ins());
        let light = Rgba::new(0xcb, 0xd8, 0xdf, 0xff);
        backdrop.dress(Look {
            splash: light,
            ..Look::default()
        });
        assert_eq!(backdrop.shown(&left, false).0, crate::BACKDROP, "none yet");

        backdrop.begin_splash();
        let (ground, image) = backdrop.shown(&left, false);
        assert_eq!(ground, clear(light), "its own ground");
        let expected = place((1920, 1080).into(), 1.0, (64, 32), Placement::Splash);
        assert_eq!(image.map(|image| image.placed), expected, "centred");
        let built = &backdrop.screens[0];
        let tint = |built: &Option<Built>| built.as_ref().map(|built| built.picture.pixels[0]);
        assert_eq!(tint(&built.splash), Some(0x10), "navy, on its light ground");
        assert_eq!(
            tint(&built.lock),
            Some(0xf0),
            "and white on the lock's dark one"
        );
        let (ground, _) = backdrop.shown(&left, true);
        assert_eq!(
            ground,
            clear(Palette::default()[Role::Lock]),
            "locked, the lock"
        );

        assert!(backdrop.splash_done_on(&left), "a wallpaper on the left");
        assert!(!backdrop.splash_done_on(&left), "said once");
        let (ground, image) = backdrop.shown(&left, false);
        assert_eq!(ground, crate::BACKDROP, "done there");
        assert!(image.is_none());
        assert!(backdrop.screens[0].splash.is_none(), "its buffer dropped");
        assert_eq!(
            backdrop.shown(&right, false).0,
            clear(light),
            "still up there"
        );

        let again = output("DP-1");
        backdrop.screens([
            (again.clone(), (1920, 1080).into(), 1.0),
            (right.clone(), (1280, 1024).into(), 1.0),
        ]);
        assert!(!backdrop.splash_on(&again), "plugged in again, not back");

        assert!(backdrop.end_splash(), "the right was still showing it");
        assert_eq!(backdrop.shown(&right, false).0, crate::BACKDROP);
        assert!(
            backdrop
                .screens
                .iter()
                .all(|screen| screen.splash.is_none())
        );
        assert!(!backdrop.end_splash(), "over already");
    }
}
