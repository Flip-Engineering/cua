// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Flip Engineering

//! Hand cursor themes (profile `flip-hand-v1`).
//!
//! A hand theme draws a character's own hand as the agent cursor. Its
//! animations hold image layers ([`crate::CompiledGeometry::Image`]) of that
//! hand, and each of the twelve actions holds a hand pose: `navigate` points,
//! `click` taps, `drag` grips, `text` rests and `idle` drums its fingers.
//!
//! The overlay picks the pose from the cursor's own state: the hand grips
//! while a button is held, taps when a click lands, points while it travels,
//! and otherwise shows the pose of the agent's current action. Between the
//! start of a click action and the click itself the hand points. A hand theme
//! paints upright at its authored size, with no halo, heading turn or click
//! squish; its poses carry their own lean and the tap is its click feedback.

use crate::render_state::RenderStateCore;
use crate::{CompiledTheme, CursorAction, CursorVisualState};

/// The profile of a hand theme. A hand theme defines all twelve actions.
pub const HAND_PROFILE: &str = "flip-hand-v1";

/// The length of the click pulse: `RenderStateCore::click_t` runs from 0 to
/// 1 over it.
const CLICK_PULSE_SECS: f64 = 0.25;

/// Whether `theme` is a hand theme.
pub fn is_hand_theme(theme: &CompiledTheme) -> bool {
    theme.profile == HAND_PROFILE
}

/// Whether `profile` names a profile whose themes define every action.
pub fn is_full_profile(profile: &str) -> bool {
    profile == crate::THEME_PROFILE || profile == HAND_PROFILE
}

/// Whether this cursor paints a hand theme.
pub fn has_hand_theme(core: &RenderStateCore) -> bool {
    core.theme.as_deref().is_some_and(is_hand_theme)
}

/// The action whose animation a hand theme paints now and the seconds into
/// that animation. `None` when the cursor paints another theme.
pub fn pose(core: &RenderStateCore) -> Option<(CursorAction, f64)> {
    if !has_hand_theme(core) {
        return None;
    }
    if core.pressed {
        return Some((CursorAction::Drag, core.visual.elapsed_secs));
    }
    if let Some(t) = core.click_t {
        return Some((CursorAction::Click, t * CLICK_PULSE_SECS));
    }
    let travelling = core
        .trajectory
        .as_ref()
        .is_some_and(|trajectory| crate::cursor_pose::travelling(trajectory, core.motion_t));
    if travelling {
        return Some((CursorAction::Navigate, core.motion_t));
    }
    Some(match core.visual.resolved_action {
        // The tap plays when the click lands; around it the hand points.
        CursorAction::Click => (CursorAction::Navigate, core.visual.elapsed_secs),
        action => (action, core.visual.elapsed_secs),
    })
}

/// The visual state a hand theme paints with: [`pose`] as the resolved
/// action. `None` when the cursor paints another theme.
pub fn visual(core: &RenderStateCore) -> Option<CursorVisualState> {
    let (action, elapsed_secs) = pose(core)?;
    Some(CursorVisualState {
        requested_action: action,
        resolved_action: action,
        elapsed_secs,
        ending_secs: None,
        ..core.visual.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme_artifact::{
        decode_theme, encode_theme, paint_compiled_theme, validate_compiled_theme,
        CompiledAnimation, CompiledDrawCommand, CompiledFrame, CompiledGeometry, CompiledTransform,
    };
    use crate::{CursorConfig, OverlayCommand};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// A `side` x `side` premultiplied image of one colour.
    fn image(side: u16, rgba: [u8; 4]) -> CompiledGeometry {
        CompiledGeometry::Image {
            size: [side, side],
            rgba: rgba.repeat(usize::from(side) * usize::from(side)),
            origin: [32.0, 32.0],
            extent: [64.0, 64.0],
        }
    }

    fn image_frame(geometry: CompiledGeometry) -> CompiledFrame {
        CompiledFrame {
            commands: vec![CompiledDrawCommand {
                geometries: vec![geometry],
                transform: CompiledTransform::default(),
                opacity: 1.0,
                fill: None,
                stroke: None,
            }],
        }
    }

    /// A hand theme whose every action is one red frame, except `idle`,
    /// which alternates red and blue.
    fn hand_theme() -> CompiledTheme {
        let red = CompiledAnimation {
            still_frame: 0,
            frames: vec![image_frame(image(8, [200, 0, 0, 255]))],
        };
        let mut actions: BTreeMap<String, CompiledAnimation> = CursorAction::ALL
            .iter()
            .map(|action| (action.as_str().to_owned(), red.clone()))
            .collect();
        actions.insert(
            "idle".into(),
            CompiledAnimation {
                still_frame: 0,
                frames: vec![
                    image_frame(image(8, [200, 0, 0, 255])),
                    image_frame(image(8, [0, 0, 200, 255])),
                ],
            },
        );
        CompiledTheme {
            id: "flip.myflip.test.v1.r2".into(),
            name: "MyFlip".into(),
            version: "1.0.0".into(),
            author: "Flip".into(),
            license: "proprietary".into(),
            profile: HAND_PROFILE.into(),
            source_hash: [3; 32],
            hotspot: [64, 64],
            actions,
        }
    }

    fn hand_core() -> RenderStateCore {
        let mut core = RenderStateCore::new(CursorConfig::default());
        core.theme = Some(Arc::new(hand_theme()));
        core.apply_command_base(
            OverlayCommand::SnapTo {
                x: 400.0,
                y: 300.0,
                heading_radians: None,
            },
            false,
            false,
        );
        core
    }

    #[test]
    fn image_themes_round_trip_and_paint_their_pixels() {
        let theme = hand_theme();
        let bytes = encode_theme(&theme).unwrap();
        assert_eq!(decode_theme(&bytes).unwrap(), theme);

        let mut target = tiny_skia::Pixmap::new(200, 200).unwrap();
        let visual = CursorVisualState {
            resolved_action: CursorAction::Text,
            requested_action: CursorAction::Text,
            ..CursorVisualState::default()
        };
        // Hotspot (64, 64) at (100, 100), upright: the image covers canvas
        // 32..96, about 10.5 points either side of the hotspot.
        paint_compiled_theme(
            &mut target,
            &theme,
            &visual,
            100.0,
            100.0,
            std::f32::consts::FRAC_PI_4,
            1.0,
            1.0,
        );
        let centre = target.pixel(100, 100).unwrap();
        assert!(centre.red() >= 195 && centre.alpha() >= 250, "{centre:?}");
        assert!(centre.green() <= 5 && centre.blue() <= 5, "{centre:?}");
        assert_eq!(target.pixel(80, 100).unwrap().alpha(), 0);
        assert_eq!(target.pixel(100, 121).unwrap().alpha(), 0);
    }

    #[test]
    fn rejects_malformed_images() {
        let mut theme = hand_theme();
        let invalid = [
            // Fewer bytes than its size.
            CompiledGeometry::Image {
                size: [8, 8],
                rgba: vec![0; 8 * 8 * 4 - 1],
                origin: [0.0, 0.0],
                extent: [8.0, 8.0],
            },
            // Colour above its alpha: not premultiplied.
            image(4, [255, 0, 0, 128]),
            // Larger than four device pixels per canvas unit.
            image(513, [0, 0, 0, 0]),
            CompiledGeometry::Image {
                size: [2, 2],
                rgba: vec![0; 16],
                origin: [0.0, 0.0],
                extent: [0.0, 8.0],
            },
        ];
        for geometry in invalid {
            theme.actions.insert(
                "text".into(),
                CompiledAnimation {
                    still_frame: 0,
                    frames: vec![image_frame(geometry)],
                },
            );
            assert!(validate_compiled_theme(&theme, true).is_err());
        }
    }

    #[test]
    fn rejects_an_image_with_vector_paint() {
        let mut theme = hand_theme();
        let mut frame = image_frame(image(4, [10, 10, 10, 255]));
        frame.commands[0].fill = Some([255, 255, 255, 255]);
        theme.actions.insert(
            "text".into(),
            CompiledAnimation {
                still_frame: 0,
                frames: vec![frame],
            },
        );
        assert!(validate_compiled_theme(&theme, true).is_err());
    }

    #[test]
    fn a_hand_theme_is_a_full_profile() {
        let mut theme = hand_theme();
        theme.actions.remove("record");
        assert!(validate_compiled_theme(&theme, is_full_profile(&theme.profile)).is_err());
    }

    #[test]
    fn the_pose_follows_the_cursor() {
        let mut core = hand_core();
        assert_eq!(
            pose(&core).map(|(action, _)| action),
            Some(CursorAction::Idle)
        );

        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Text,
                delivery: None,
                target: None,
            },
            false,
            false,
        );
        assert_eq!(core.painted_action(), CursorAction::Text);

        // A click points while it travels and taps when it lands.
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Click,
                delivery: None,
                target: None,
            },
            false,
            false,
        );
        assert_eq!(core.painted_action(), CursorAction::Navigate);
        core.apply_command_base(
            OverlayCommand::MoveTo {
                x: 900.0,
                y: 700.0,
                end_heading_radians: std::f64::consts::FRAC_PI_4,
                target: None,
            },
            false,
            false,
        );
        core.tick_motion(0.05);
        assert_eq!(core.painted_action(), CursorAction::Navigate);
        assert_eq!(core.painted_rotation(), 0.0);
        core.apply_command_base(
            OverlayCommand::ClickPulse { x: 900.0, y: 700.0 },
            false,
            false,
        );
        assert_eq!(core.painted_action(), CursorAction::Click);

        core.apply_command_base(OverlayCommand::SetPressed(true), false, false);
        assert_eq!(core.painted_action(), CursorAction::Drag);
    }

    #[test]
    fn the_idle_drum_keeps_rendering_and_other_themes_have_no_pose() {
        let core = hand_core();
        assert!(core.has_resting_motion());
        let other = RenderStateCore::new(CursorConfig::default());
        assert_eq!(pose(&other), None);
        assert_eq!(visual(&other), None);
    }
}
