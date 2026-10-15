//! The splash, by a compositor with no screen: seen in pictures of its
//! monitors, as a seat shows it from its session's first frame.
//!
//! It is the theme's `splash` colour, with Perspicax's lockup in the middle,
//! lettered in white on a dark colour and in navy on a light one. A
//! wallpaper over the whole of a monitor ends it there, and a strip on the
//! background does not; with nothing ever over it, it ends after a while.
//! It is beneath everything: a picture of one window never shows it.
//!
//! A seat puts its splash up itself; here [`Command::Splash`] does, with
//! the lockup decoded there and then. Only with the `capture` feature. Like
//! the other live tests it binds a real Wayland socket, so it needs
//! `XDG_RUNTIME_DIR`, and is `#[ignore]`d for `ci/live-tests.sh` to run.

#![cfg(feature = "capture")]

mod common;

use std::time::{Duration, Instant};

use common::{Session, until};
use perspicax_compositor::{Backend, Command, Host, Virtual};
use perspicax_index::{Shot, ShotTarget};
use perspicax_policy::{Access, Builtin, Palette, Place, Shape, Side};
use smithay_client_toolkit::shell::wlr_layer::Layer;

/// The colours of the lockup, and of the grounds it is shown on.
const NAVY: [u8; 3] = [0x0c, 0x24, 0x40];
const TERRACOTTA: [u8; 3] = [0xc0, 0x6a, 0x3c];
const SKY: [u8; 3] = [0x7b, 0xae, 0xcd];
const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const PALE: [u8; 3] = [0xcb, 0xd8, 0xdf];
/// The grey where nothing is drawn: `BACKDROP`.
const GREY: [u8; 3] = [31, 31, 36];

/// ARGB, as the client writes it.
const WALLPAPER: u32 = 0xff22_8844;
const STRIP: u32 = 0xff88_2244;

/// An ARGB colour's red, green and blue, as a picture has them.
fn rgb(argb: u32) -> [u8; 3] {
    let [b, g, r, _] = argb.to_le_bytes();
    [r, g, b]
}

fn pixel(shot: &Shot, x: usize, y: usize) -> [u8; 4] {
    let at = (y * shot.width as usize + x) * 4;
    shot.rgba[at..at + 4].try_into().unwrap()
}

/// Whether `seen` is `colour`, give or take what blending and rounding
/// leave.
fn near(seen: [u8; 4], colour: [u8; 3]) -> bool {
    seen[..3]
        .iter()
        .zip(colour)
        .all(|(&seen, wanted)| seen.abs_diff(wanted) <= 3)
}

/// How many pixels of `shot` are `colour`.
fn count(shot: &Shot, colour: [u8; 3]) -> usize {
    shot.rgba
        .chunks_exact(4)
        .filter(|&seen| near(seen.try_into().unwrap(), colour))
        .count()
}

/// The box round every pixel of `shot` that is not `ground`: where
/// something is drawn on it, as `(left, top, right, bottom)`.
fn drawn_on(shot: &Shot, ground: [u8; 3]) -> (usize, usize, usize, usize) {
    let (width, height) = (shot.width as usize, shot.height as usize);
    let mut drawn = (width, height, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if !near(pixel(shot, x, y), ground) {
                drawn = (
                    drawn.0.min(x),
                    drawn.1.min(y),
                    drawn.2.max(x),
                    drawn.3.max(y),
                );
            }
        }
    }
    drawn
}

fn splash(lasts: Duration, palette: Palette) -> Command {
    Command::Splash { lasts, palette }
}

/// Long enough never to end during a test that does not wait for it.
const LONG: Duration = Duration::from_secs(600);

fn picture(host: &Host, output: Option<&str>) -> Shot {
    host.capture(ShotTarget::Output(output.map(str::to_owned)))
        .expect("a picture")
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn the_splash_is_its_colour_with_the_lockup_in_the_middle() {
    let session = Session::start("splash-shown", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    let before = picture(&host, None);
    assert!(near(pixel(&before, 5, 5), GREY), "no splash unasked");

    session.command(splash(LONG, Palette::default()));
    let shot = picture(&host, None);
    for (x, y) in [(5, 5), (794, 5), (5, 594), (794, 594)] {
        assert!(
            near(pixel(&shot, x, y), NAVY),
            "the splash's navy at {x}, {y}"
        );
    }
    for (colour, name) in [(TERRACOTTA, "terracotta"), (SKY, "sky"), (WHITE, "white")] {
        assert!(count(&shot, colour) > 200, "the lockup's {name}");
    }
    // A quarter of the height, its shape kept: 189 by 150, in the middle.
    let (left, top, right, bottom) = drawn_on(&shot, NAVY);
    assert!(
        right - left < 189 && bottom - top < 150,
        "{left} {top} {right} {bottom}"
    );
    assert!(
        (left + right).abs_diff(799) <= 6,
        "centred across: {left} to {right}"
    );
    assert!(
        (top + bottom).abs_diff(599) <= 6,
        "centred down: {top} to {bottom}"
    );

    session.stop(());
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn on_a_light_splash_the_lockup_is_lettered_navy() {
    let session = Session::start("splash-light", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    session.command(splash(LONG, Builtin::BreezeLight.palette()));
    let shot = picture(&host, None);
    assert!(
        near(pixel(&shot, 5, 5), PALE),
        "Breeze Light's, as its lock is"
    );
    assert!(count(&shot, NAVY) > 200, "navy lettering");
    assert_eq!(count(&shot, WHITE), 0, "and no white");
    assert!(count(&shot, TERRACOTTA) > 200, "the mark as it always is");
    session.stop(());
}

/// Two monitors side by side, HEADLESS-2 to the right of HEADLESS-1.
fn two_monitors() -> Backend {
    Backend::Headless {
        outputs: vec![
            Virtual::numbered(1, (800, 600)),
            Virtual {
                place: Place::Beside {
                    side: Side::RightOf,
                    of: "HEADLESS-1".to_owned(),
                    offset: 0,
                },
                ..Virtual::numbered(2, (800, 600))
            },
        ],
        workspaces: Shape::default(),
        access: Access::open(),
        person: false,
        opacity: Default::default(),
        agent: Default::default(),
    }
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_wallpaper_over_a_whole_monitor_ends_it_there_and_only_there() {
    let session = Session::start("splash-ended", two_monitors());
    let host = Host::new(&session.facts, &session.requests);
    session.command(splash(LONG, Palette::default()));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_wallpaper(&qh, WALLPAPER);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let names = ["HEADLESS-1", "HEADLESS-2"];
    let covered = names
        .into_iter()
        .find(|name| near(pixel(&picture(&host, Some(name)), 5, 300), rgb(WALLPAPER)))
        .expect("the wallpaper on one monitor");
    let other = names.into_iter().find(|name| *name != covered).unwrap();
    let shot = picture(&host, Some(other));
    assert!(
        near(pixel(&shot, 5, 300), NAVY),
        "the splash still on {other}"
    );

    // The wallpaper goes, and the splash does not come back from under it.
    desk.layers.clear();
    queue.roundtrip(&mut desk).expect("destroyed");
    let shot = picture(&host, Some(covered));
    let below = pixel(&shot, 5, 300);
    assert!(near(below, GREY), "done on {covered}: {below:?}");
    let shot = picture(&host, Some(other));
    assert!(near(pixel(&shot, 5, 300), NAVY), "and still up on {other}");

    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_background_strip_leaves_the_splash_up() {
    let session = Session::start("splash-strip", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    session.command(splash(LONG, Palette::default()));
    let (mut desk, mut queue, qh, _) = session.client();
    desk.open_strip(&qh, Layer::Background, "strip", 40, STRIP);
    until(&mut queue, &mut desk, |desk| desk.layers_drawn == 1);
    let shot = picture(&host, None);
    assert!(near(pixel(&shot, 5, 5), rgb(STRIP)), "the strip, drawn");
    assert!(
        near(pixel(&shot, 5, 300), NAVY),
        "and the splash under the rest"
    );
    session.stop((desk, queue));
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn with_nothing_over_it_the_splash_ends_after_a_while() {
    let session = Session::start("splash-over", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    session.command(splash(Duration::from_millis(300), Palette::default()));
    assert!(near(pixel(&picture(&host, None), 5, 5), NAVY), "up");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let shot = picture(&host, None);
        if near(pixel(&shot, 5, 5), GREY) {
            assert_eq!(count(&shot, TERRACOTTA), 0, "the lockup gone with it");
            break;
        }
        assert!(Instant::now() < deadline, "still up after ten seconds");
        std::thread::sleep(Duration::from_millis(50));
    }
    session.stop(());
}

#[test]
#[ignore = "binds a real Wayland socket; needs XDG_RUNTIME_DIR"]
fn a_picture_of_one_window_never_shows_the_splash() {
    let session = Session::start("splash-window", Backend::headless((800, 600)));
    let host = Host::new(&session.facts, &session.requests);
    session.command(splash(LONG, Palette::default()));
    let (mut desk, mut queue, qh, _) = session.client();
    // Clear all over, so that what is behind it shows wherever it is.
    desk.open_coloured(&qh, "clear", "clear", 0x0000_0000);
    until(&mut queue, &mut desk, |desk| desk.drawn == 1);
    let facts = session.wait_for(|facts| facts.surfaces().iter().any(|surface| surface.mapped));
    let window = facts
        .surfaces()
        .iter()
        .find(|surface| surface.title.as_deref() == Some("clear"))
        .expect("the window");

    let alone = host
        .capture(ShotTarget::Window(window.id))
        .expect("a picture of the window");
    assert_eq!(count(&alone, NAVY), 0, "not the splash's colour");
    assert!(near(pixel(&alone, 1, 1), GREY), "the plain grey behind it");
    let monitor = picture(&host, None);
    let (x, y) = (
        window.geometry.x0 as usize + 2,
        window.geometry.y0 as usize + 2,
    );
    assert!(
        near(pixel(&monitor, x, y), NAVY),
        "on the monitor, the splash through it"
    );

    session.stop((desk, queue));
}
