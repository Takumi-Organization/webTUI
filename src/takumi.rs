// Copiado de takumi-design (dist/takumi.rs, generado desde tokens.json): estilo visual Takumi.
// Origen: https://github.com/Takumi-Organization/takumi-design. No editar a mano; regenerar allí.
// Generado desde tokens.json por scripts/generate.py. No editar a mano.
use ratatui::style::Color;

pub mod oscuro {
    use super::Color;
    pub const FONDO: Color = Color::Rgb(0, 0, 0);
    pub const PANEL: Color = Color::Rgb(8, 8, 8);
    pub const ELEVADO: Color = Color::Rgb(17, 17, 17);
    pub const TINTA: Color = Color::Rgb(242, 242, 242);
    pub const TINTA_SUAVE: Color = Color::Rgb(201, 201, 201);
    pub const TINTA_APAGADA: Color = Color::Rgb(140, 140, 140);
    pub const TINTA_TENUE: Color = Color::Rgb(74, 74, 74);
    pub const LINEA: Color = Color::Rgb(28, 28, 28);
    pub const LINEA_ALTA: Color = Color::Rgb(58, 58, 58);
    pub const INVERSO_FONDO: Color = Color::Rgb(242, 242, 242);
    pub const INVERSO_TINTA: Color = Color::Rgb(0, 0, 0);
}

pub mod claro {
    use super::Color;
    pub const FONDO: Color = Color::Rgb(244, 244, 244);
    pub const PANEL: Color = Color::Rgb(255, 255, 255);
    pub const ELEVADO: Color = Color::Rgb(235, 235, 235);
    pub const TINTA: Color = Color::Rgb(10, 10, 10);
    pub const TINTA_SUAVE: Color = Color::Rgb(51, 51, 51);
    pub const TINTA_APAGADA: Color = Color::Rgb(107, 107, 107);
    pub const TINTA_TENUE: Color = Color::Rgb(176, 176, 176);
    pub const LINEA: Color = Color::Rgb(224, 224, 224);
    pub const LINEA_ALTA: Color = Color::Rgb(192, 192, 192);
    pub const INVERSO_FONDO: Color = Color::Rgb(10, 10, 10);
    pub const INVERSO_TINTA: Color = Color::Rgb(244, 244, 244);
}

pub const GRISES: [Color; 6] = [
    Color::Rgb(242, 242, 242),
    Color::Rgb(201, 201, 201),
    Color::Rgb(140, 140, 140),
    Color::Rgb(94, 94, 94),
    Color::Rgb(58, 58, 58),
    Color::Rgb(28, 28, 28),
];
pub const ESPACIADO_ETIQUETA: usize = 1; // las etiquetas van en MAYÚSCULAS y separadas
