//! A person's own pictures for the titlebar's buttons: `[decorations.icons]`.
//!
//! They are read at every save, on a thread of their own, as the lock
//! screen's logo is: a picture is a file, and a slow disk or a hung home
//! folder must stall nothing. Until a reading arrives, the buttons keep what
//! they showed. A face whose file cannot be read is drawn as its built-in
//! shape, with one warning naming its key and its path, never as an empty
//! button.
//!
//! A PNG is shrunk to the room its button has, never enlarged; an SVG, in a
//! build with the `svg` feature, is drawn at exactly that room, so it is
//! crisp at any scale. Either is kept at that size: a titlebar is drawn
//! again at every new title.

use std::{
    io::BufReader,
    path::{Path, PathBuf},
    rc::Rc,
};

use perspicax_policy::Face;
use smithay::reexports::calloop::channel::Sender;

use crate::picture::{Picture, Unread, decode, shrink};

/// A face's file, read: a picture, or a drawing to draw at whatever size
/// its button is.
enum Read {
    Png(Picture),
    #[cfg(feature = "svg")]
    Svg(Box<resvg::usvg::Tree>),
}

/// What a face is drawn from.
enum Source {
    Png(Rc<Picture>),
    #[cfg(feature = "svg")]
    Svg(Box<resvg::usvg::Tree>),
}

impl From<Read> for Source {
    fn from(read: Read) -> Self {
        match read {
            Read::Png(picture) => Self::Png(Rc::new(picture)),
            #[cfg(feature = "svg")]
            Read::Svg(tree) => Self::Svg(tree),
        }
    }
}

/// What a reading found: each face's file, and what came of reading it.
pub(super) struct Arrived {
    /// The asking it answers.
    asked: u64,
    read: Vec<(Face, PathBuf, Result<Read, String>)>,
    tint: bool,
}

/// The pictures the titlebar's buttons are drawn from, as last read.
pub(super) struct Icons {
    faces: Vec<(Face, Source)>,
    /// Whether each is a mask, drawn in the bar's ink.
    tint: bool,
    /// Counts each change to what is shown, so that a title drawn before it
    /// is drawn again.
    generation: u64,
    /// Counts each asking, so a slow reading of an older save never
    /// overwrites a newer one.
    asked: u64,
    /// Faces shrunk to the room they had, by face and room.
    shrunk: Vec<(Face, u32, Rc<Picture>)>,
}

impl Icons {
    /// None yet: every button its built-in shape.
    pub(super) fn new() -> Self {
        Self {
            faces: Vec::new(),
            tint: true,
            generation: 0,
            asked: 0,
            shrunk: Vec::new(),
        }
    }

    /// Read `icons` again, as a save asks, on a thread that sends what it
    /// read to `to`: a relative path beside the config at `config`, and `~`
    /// in the home folder. With no files at all there is nothing to read,
    /// and the built-in shapes are back at once.
    pub(super) fn read(
        &mut self,
        icons: &perspicax_config::Icons,
        config: Option<&Path>,
        to: &Sender<Arrived>,
    ) {
        self.asked += 1;
        if icons.files.is_empty() {
            self.show(Vec::new(), icons.tint);
            return;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let files: Vec<(Face, PathBuf)> = icons
            .files
            .iter()
            .map(|(face, written)| {
                (
                    *face,
                    perspicax_config::locate(written, config, home.as_deref()),
                )
            })
            .collect();
        let (asked, tint, to) = (self.asked, icons.tint, to.clone());
        let reading = std::thread::Builder::new()
            .name("perspicax-icons".to_owned())
            .spawn(move || {
                let read = files
                    .into_iter()
                    .map(|(face, path)| {
                        let read = read(&path);
                        (face, path, read)
                    })
                    .collect();
                // Unsent only once the session has ended.
                let _ = to.send(Arrived { asked, read, tint });
            });
        if let Err(error) = reading {
            tracing::warn!(
                %error,
                "could not start a thread to read the titlebar's icons; its buttons keep the \
                 shapes they have"
            );
        }
    }

    /// Show what a reading found, if it answers the latest asking. Whether
    /// anything is to be drawn differently.
    pub(super) fn arrived(&mut self, arrived: Arrived) -> bool {
        if arrived.asked != self.asked {
            return false;
        }
        let faces = arrived
            .read
            .into_iter()
            .filter_map(|(face, path, read)| match read {
                Ok(read) => Some((face, Source::from(read))),
                Err(error) => {
                    tracing::warn!(
                        icon = face.key(),
                        path = %path.display(),
                        %error,
                        "an icon could not be read; its button keeps its built-in shape"
                    );
                    None
                }
            })
            .collect();
        self.show(faces, arrived.tint);
        true
    }

    fn show(&mut self, faces: Vec<(Face, Source)>, tint: bool) {
        self.faces = faces;
        self.tint = tint;
        self.shrunk.clear();
        self.generation += 1;
    }

    /// Which change to what is shown this is, for a title's key.
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a picture is drawn in the bar's ink, by its alpha.
    pub(super) fn tint(&self) -> bool {
        self.tint
    }

    /// `face`'s picture, fitted to a square `room` pixels a side, or `None`
    /// for its built-in shape: a PNG shrunk, never enlarged, and an SVG
    /// drawn that size.
    pub(super) fn face(&mut self, face: Face, room: u32) -> Option<Rc<Picture>> {
        if let Some((.., shrunk)) = self
            .shrunk
            .iter()
            .find(|(shown, at, _)| *shown == face && *at == room)
        {
            return Some(Rc::clone(shrunk));
        }
        let (_, source) = self.faces.iter().find(|(shown, _)| *shown == face)?;
        let fitted = match source {
            Source::Png(picture) => fit(picture, room)?,
            #[cfg(feature = "svg")]
            Source::Svg(tree) => Rc::new(draw(tree, room)?),
        };
        self.shrunk.push((face, room, Rc::clone(&fitted)));
        Some(fitted)
    }

    /// Show `picture` as `face`, as a reading would, for a test.
    #[cfg(test)]
    pub(super) fn put(&mut self, face: Face, picture: Picture, tint: bool) {
        let mut faces = std::mem::take(&mut self.faces);
        faces.retain(|(shown, _)| *shown != face);
        faces.push((face, Source::Png(Rc::new(picture))));
        self.show(faces, tint);
    }
}

/// Read the file at `path`: an SVG by its name, in a build that draws SVG,
/// and otherwise a PNG. The config refuses an SVG a build cannot draw, so
/// one is only read here as a PNG, and refused as not one, if the config
/// was read by a build that could.
fn read(path: &Path) -> Result<Read, String> {
    #[cfg(feature = "svg")]
    if perspicax_config::is_svg(path) {
        return std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| svg(&bytes).map_err(|error| error.to_string()))
            .map(|tree| Read::Svg(Box::new(tree)));
    }
    std::fs::File::open(path)
        .map_err(Unread::from)
        .and_then(|file| decode(BufReader::new(file)))
        .map(Read::Png)
        .map_err(|error| error.to_string())
}

/// An SVG, parsed. Nothing outside the file is read, as the shell reads
/// its icons: an image it names by path is left out, for an icon needs
/// none, and a path could name anything on the disk, `/dev/zero` among it.
#[cfg(feature = "svg")]
fn svg(bytes: &[u8]) -> Result<resvg::usvg::Tree, resvg::usvg::Error> {
    use resvg::usvg::{ImageHrefResolver, Options, Tree};

    let options = Options {
        image_href_resolver: ImageHrefResolver {
            resolve_data: ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Options::default()
    };
    Tree::from_data(bytes, &options)
}

/// `tree` drawn to fit a square `room` pixels a side, centred, its shape
/// kept: premultiplied ARGB8888, blue first, as every picture here is.
#[cfg(feature = "svg")]
fn draw(tree: &resvg::usvg::Tree, room: u32) -> Option<Picture> {
    use resvg::tiny_skia::{Pixmap, Transform};

    let mut drawn = Pixmap::new(room, room)?;
    let (width, height) = (tree.size().width(), tree.size().height());
    let side = room as f32;
    let scale = (side / width).min(side / height);
    let at = Transform::from_row(
        scale,
        0.0,
        0.0,
        scale,
        (side - width * scale) / 2.0,
        (side - height * scale) / 2.0,
    );
    resvg::render(tree, at, &mut drawn.as_mut());
    // tiny-skia keeps red first; a buffer here is blue first.
    let mut pixels = drawn.take();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Picture {
        size: (room, room),
        pixels,
    })
}

/// `picture` shrunk to fit a square `room` pixels a side, keeping its
/// shape, or as it is if it fits already.
fn fit(picture: &Rc<Picture>, room: u32) -> Option<Rc<Picture>> {
    let (width, height) = picture.size;
    if width <= room && height <= room {
        return Some(Rc::clone(picture));
    }
    let longer = width.max(height);
    let size = (
        (width * room / longer).max(1),
        (height * room / longer).max(1),
    );
    let pixels = shrink(&picture.pixels, picture.size, size)?;
    Some(Rc::new(Picture { size, pixels }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(size: (u32, u32)) -> Picture {
        Picture {
            size,
            pixels: [0, 0, 0xff, 0xff].repeat((size.0 * size.1) as usize),
        }
    }

    /// Shrunk to its room, its shape kept; one that fits is shown at its
    /// own size, never enlarged.
    #[test]
    fn a_picture_is_shrunk_to_its_room_and_never_enlarged() {
        let mut icons = Icons::new();
        icons.put(Face::Close, solid((100, 50)), true);
        assert_eq!(icons.face(Face::Close, 14).map(|p| p.size), Some((14, 7)));
        icons.put(Face::Close, solid((4, 4)), true);
        assert_eq!(icons.face(Face::Close, 14).map(|p| p.size), Some((4, 4)));
        assert!(
            icons.face(Face::Restore, 14).is_none(),
            "no file, no picture"
        );
    }

    /// An SVG is drawn at its button's own size, whatever size it says it
    /// is: as large as the room, at any room, and red where it is red.
    #[cfg(feature = "svg")]
    #[test]
    fn an_svg_icon_is_drawn_at_the_buttons_own_size_at_any_scale() {
        let red = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <rect width="10" height="10" fill="#ff0000"/></svg>"##;
        let mut icons = Icons::new();
        icons.faces = vec![(
            Face::Close,
            Source::Svg(Box::new(svg(red).expect("parsed"))),
        )];
        for room in [14, 29, 58] {
            let picture = icons.face(Face::Close, room).expect("drawn");
            assert_eq!(picture.size, (room, room), "drawn {room} a side, not 10");
            let middle = ((room / 2 * room + room / 2) * 4) as usize;
            assert_eq!(
                picture.pixels[middle..middle + 4],
                [0, 0, 0xff, 0xff],
                "red, blue first"
            );
            let corner = ((room + 1) * 4) as usize;
            assert_eq!(
                picture.pixels[corner + 3],
                0xff,
                "filling the room, not drawn at its own 10 px in the middle of it"
            );
        }
        assert!(svg(b"<svg").is_err(), "not an SVG it can read");
    }

    /// A reading answers the asking it was started for, or nothing: a slow
    /// one from an older save is thrown away. One that could not read a
    /// file leaves that face its built-in shape.
    #[test]
    fn only_the_latest_reading_is_shown_and_an_unread_file_is_its_shape() {
        let mut icons = Icons::new();
        icons.asked = 2;
        let older = Arrived {
            asked: 1,
            read: vec![(
                Face::Close,
                PathBuf::from("close.png"),
                Ok(Read::Png(solid((4, 4)))),
            )],
            tint: true,
        };
        assert!(!icons.arrived(older), "an older asking's");
        assert!(icons.face(Face::Close, 14).is_none());
        let latest = Arrived {
            asked: 2,
            read: vec![
                (
                    Face::Close,
                    PathBuf::from("close.png"),
                    Ok(Read::Png(solid((4, 4)))),
                ),
                (
                    Face::Shade,
                    PathBuf::from("gone.png"),
                    Err("no such file".to_owned()),
                ),
            ],
            tint: false,
        };
        let before = icons.generation();
        assert!(icons.arrived(latest));
        assert!(icons.generation() > before, "titles are drawn again");
        assert!(icons.face(Face::Close, 14).is_some());
        assert!(icons.face(Face::Shade, 14).is_none(), "its built-in shape");
        assert!(!icons.tint());
    }
}
