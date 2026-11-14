//! Small flags drawn by Keel. The text font is not asked to paint emoji.

use iced::widget::{column, container, row, text, Space};
use iced::{Background, Color, Element};

pub fn icon<'a, Message: 'a>(code: &str) -> Element<'a, Message> {
    match spec(code) {
        Some(Spec::H(colors)) => stripes(true, colors),
        Some(Spec::V(colors)) => stripes(false, colors),
        Some(Spec::Us) => united_states(),
        None => badge(code),
    }
}

enum Spec {
    H(&'static [&'static str]),
    V(&'static [&'static str]),
    Us,
}

fn spec(code: &str) -> Option<Spec> {
    Some(match code {
        "US" => Spec::Us,
        "DE" => Spec::H(&["#000000", "#DD0000", "#FFCE00"]),
        "AT" | "LV" | "MC" | "PE" => Spec::H(&["#ED2939", "#FFFFFF", "#ED2939"]),
        "BG" => Spec::H(&["#FFFFFF", "#00966E", "#D62612"]),
        "EE" => Spec::H(&["#0072CE", "#000000", "#FFFFFF"]),
        "HU" => Spec::H(&["#CE2939", "#FFFFFF", "#477050"]),
        "LT" => Spec::H(&["#FDB913", "#006A44", "#C1272D"]),
        "LU" => Spec::H(&["#ED2939", "#FFFFFF", "#00A1DE"]),
        "NL" => Spec::H(&["#AE1C28", "#FFFFFF", "#21468B"]),
        "RU" => Spec::H(&["#FFFFFF", "#0039A6", "#D52B1E"]),
        "SI" => Spec::H(&["#FFFFFF", "#003DA5", "#ED1C24"]),
        "UA" => Spec::H(&["#005BBB", "#FFD500"]),
        "AM" => Spec::H(&["#D90012", "#0033A0", "#F2A800"]),
        "BO" => Spec::H(&["#D52B1E", "#F9E300", "#007934"]),
        "CO" => Spec::H(&["#FCD116", "#003893", "#CE1126"]),
        "EC" => Spec::H(&["#FFD100", "#034EA2", "#ED1C24"]),
        "TH" => Spec::H(&["#A51931", "#F4F5F8", "#2D2A4A", "#F4F5F8", "#A51931"]),
        "YE" => Spec::H(&["#CE1126", "#FFFFFF", "#000000"]),
        "MX" => Spec::V(&["#006847", "#FFFFFF", "#CE1126"]),
        "AR" => Spec::H(&["#74ACDF", "#FFFFFF", "#74ACDF"]),
        "ES" => Spec::H(&["#AA151B", "#F1BF00", "#AA151B"]),
        "PL" => Spec::H(&["#FFFFFF", "#DC143C"]),
        "CZ" => Spec::H(&["#FFFFFF", "#D7141A"]),
        "DK" => Spec::H(&["#C60C30", "#FFFFFF", "#C60C30"]),
        "FI" => Spec::H(&["#FFFFFF", "#003580", "#FFFFFF"]),
        "SE" | "NO" => Spec::H(&["#006AA7", "#FECC00", "#006AA7"]),
        "CH" => Spec::H(&["#FF0000"]),
        "JP" | "KR" => Spec::H(&["#FFFFFF"]),
        "BD" => Spec::H(&["#006A4E"]),
        "CN" | "VN" | "TR" | "TW" => Spec::H(&["#DE2910"]),
        "PT" => Spec::V(&["#006600", "#FF0000"]),
        "GB" | "AU" | "NZ" => Spec::H(&["#012169"]),
        "BR" => Spec::H(&["#009C3B"]),
        "IN" => Spec::H(&["#FF9933", "#FFFFFF", "#138808"]),
        "IE" => Spec::V(&["#169B62", "#FFFFFF", "#FF883E"]),
        "IT" => Spec::V(&["#009246", "#FFFFFF", "#CE2B37"]),
        "BE" => Spec::V(&["#000000", "#FAE042", "#ED2939"]),
        "FR" => Spec::V(&["#0055A4", "#FFFFFF", "#EF4135"]),
        "RO" => Spec::V(&["#002B7F", "#FCD116", "#CE1126"]),
        "NG" => Spec::V(&["#008751", "#FFFFFF", "#008751"]),
        "ML" => Spec::V(&["#14B53A", "#FCD116", "#CE1126"]),
        "GN" => Spec::V(&["#CE1126", "#FCD116", "#009460"]),
        "CI" => Spec::V(&["#FF8200", "#FFFFFF", "#009A44"]),
        "ZA" => Spec::H(&["#007A4D", "#FFB612", "#DE3831"]),
        "ID" => Spec::H(&["#CE1126", "#FFFFFF"]),
        "SG" => Spec::H(&["#EF3340", "#FFFFFF"]),
        "MY" => Spec::H(&["#CC0001", "#FFFFFF", "#CC0001", "#FFFFFF"]),
        "PH" => Spec::H(&["#0038A8", "#CE1126"]),
        "IL" => Spec::H(&["#FFFFFF", "#0038B8", "#FFFFFF"]),
        "GR" => Spec::H(&["#0D5EAF", "#FFFFFF", "#0D5EAF", "#FFFFFF", "#0D5EAF"]),
        _ => return None,
    })
}

fn stripes<'a, Message: 'a>(
    horizontal: bool,
    colors: &'static [&'static str],
) -> Element<'a, Message> {
    let count = colors.len().max(1) as f32;
    if horizontal {
        let height = 14.0 / count;
        let mut bars = column![];
        for color in colors {
            bars = bars.push(block(parse(color), 22.0, height));
        }
        bars.into()
    } else {
        let width = 22.0 / count;
        let mut bars = row![];
        for color in colors {
            bars = bars.push(block(parse(color), width, 14.0));
        }
        bars.into()
    }
}

fn united_states<'a, Message: 'a>() -> Element<'a, Message> {
    row![
        block(parse("#3C3B6E"), 9.0, 14.0),
        column![
            block(parse("#B22234"), 13.0, 2.8),
            block(parse("#FFFFFF"), 13.0, 2.8),
            block(parse("#B22234"), 13.0, 2.8),
            block(parse("#FFFFFF"), 13.0, 2.8),
            block(parse("#B22234"), 13.0, 2.8),
        ]
    ]
    .into()
}

fn badge<'a, Message: 'a>(code: &str) -> Element<'a, Message> {
    container(text(code.to_owned()).size(9))
        .width(22)
        .height(14)
        .align_x(iced::Center)
        .align_y(iced::Center)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgb8(0x2a, 0x2e, 0x38))),
            ..Default::default()
        })
        .into()
}

fn block<'a, Message: 'a>(color: Color, width: f32, height: f32) -> Element<'a, Message> {
    container(Space::new().width(width).height(height))
        .style(move |_theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

fn parse(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).unwrap_or(0x333333);
    Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8)
}
