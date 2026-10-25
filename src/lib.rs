//! webTUI: a browser for the terminal. Firefox (headless, over Marionette) is the engine;
//! the page is drawn as terminal cells: colours and images from a screenshot, text as text.

pub mod app;
pub mod browser;
pub mod firefox;
pub mod marionette;
pub mod page;
pub mod render;
pub mod url;
