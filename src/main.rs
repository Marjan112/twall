#![allow(clippy::collapsible_if)]

use std::{
    env,
    io,
    time::{Duration, Instant},
    path::PathBuf, fs,
    process::{Command, Stdio},
    thread, sync::{Arc, mpsc::{self, SyncSender, Receiver}},
    collections::HashMap
};
use ratatui::{
    crossterm::event::{self, KeyCode},
    Frame,
    layout::{Layout, Constraint, HorizontalAlignment, Rect},
    widgets::{List, ListState, ListItem, Block},
    style::{Color, Modifier, Style},
    text::Line,
    DefaultTerminal
};
use image::DynamicImage;
use ratatui_image::{FilterType, Resize, StatefulImage, picker::Picker, protocol::StatefulProtocol};
use walkdir::WalkDir;
use ratatui_textarea::TextArea;

// TODO: If an error occurs while trying to set a wallpaper display it in a pop up

#[derive(PartialEq)]
enum DisplayServer {
    X11,
    Wayland,
    Unknown
}

fn detect_display_server() -> DisplayServer {
    if env::var("WAYLAND_DISPLAY").is_ok() {
        return DisplayServer::Wayland;
    }

    if let Ok(session_type) = env::var("XDG_SESSION_TYPE") {
        match session_type.as_str() {
            "x11" => return DisplayServer::X11,
            "wayland" => return DisplayServer::Wayland,
            _ => ()
        }
    }

    if env::var("DISPLAY").is_ok() {
        return DisplayServer::X11;
    }

    DisplayServer::Unknown
}

enum Mode {
    Normal,
    Search
}

struct App {
    wallpaper_list_state: ListState,
    wallpapers: Vec<PathBuf>,
    filtered_wallpapers: Vec<PathBuf>,
    image_state: StatefulProtocol,
    dot_wallpaper: PathBuf, // ~/.wallpaper file where we store the path of the current wallpaper
    indicator: Option<char>,
    message: String,
    mode: Mode,
    search_input: TextArea<'static>,
    current_wallpaper: Option<PathBuf>,
    path_tx: SyncSender<PathBuf>,
    image_rx: Receiver<StatefulProtocol>,
    preview_update_timer: Instant,
    shift_g_pressed: bool,
    display_server: DisplayServer
}

impl App {
    fn new() -> io::Result<Self> {
        let display_server = detect_display_server();
        if display_server == DisplayServer::Unknown {
            return Err(io::Error::other("Unknown display server"));
        }

        let picker = Picker::from_query_stdio().map_err(io::Error::other)?;
        let image_state = picker.new_resize_protocol(DynamicImage::default());

        let (path_tx, path_rx) = mpsc::sync_channel::<PathBuf>(1);
        let (image_tx, image_rx) = mpsc::sync_channel::<StatefulProtocol>(1);

        thread::spawn(move || {
            let mut cache: HashMap<PathBuf, Arc<DynamicImage>> = HashMap::new();

            while let Ok(path) = path_rx.recv() {
                let preview = {
                    if let Some(cached_image) = cache.get(&path) {
                        cached_image.clone()
                    } else {
                        match image::ImageReader::open(&path).and_then(|r| r.decode().map_err(io::Error::other)) {
                            Ok(image) => {
                                let thumbnail = Arc::new(image.thumbnail(800, 600));
                                cache.insert(path.clone(), thumbnail.clone());
                                thumbnail
                            }
                            Err(_) => continue
                        }
                    }
                };

                let _ = image_tx.send(picker.new_resize_protocol((*preview).clone()));
            }
        });

        let exts = ["jpg", "jpeg", "png", "gif", "webp"];
        let mut wallpapers = Vec::new();

        for entry in WalkDir::new("/usr/share/backgrounds/")
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.into_path();

            if path.is_file() && let Some(ext) = path.extension().and_then(|os_str| os_str.to_str()) && exts.contains(&ext.to_lowercase().as_str()) {
                wallpapers.push(path);
            }
        }

        let dot_wallpaper = PathBuf::from(format!("{}/.wallpaper", env::home_dir().unwrap().display()));

        let mut wallpaper_list_state = ListState::default().with_selected(Some(0));

        let mut current_wallpaper = None;

        if let Ok(wallpaper) = fs::read_to_string(&dot_wallpaper) {
            let wallpaper = PathBuf::from(&wallpaper);
            if let Some(index) = wallpapers.iter().position(|w| w == &wallpaper) {
                wallpaper_list_state = wallpaper_list_state.with_selected(Some(index));
                current_wallpaper = Some(wallpaper);
            }
        }

        let mut search_input = TextArea::default();
        search_input.set_cursor_line_style(Style::default().white());
        search_input.set_placeholder_text("Search wallpaper...");

        let mut app = Self {
            wallpaper_list_state,
            wallpapers: wallpapers.clone(),
            filtered_wallpapers: wallpapers,
            image_state,
            dot_wallpaper,
            indicator: None,
            message: String::new(),
            mode: Mode::Normal,
            search_input,
            current_wallpaper,
            path_tx,
            image_rx,
            preview_update_timer: Instant::now(),
            shift_g_pressed: false,
            display_server
        };

        app.update_selected_image(false);
        Ok(app)
    }

    fn update_selected_image(&mut self, last: bool) {
        if last && let Some(path) = self.filtered_wallpapers.last() {
            let _ = self.path_tx.try_send(path.clone());
        } else if let Some(index) = self.wallpaper_list_state.selected() && let Some(path) = self.filtered_wallpapers.get(index) {
            let _ = self.path_tx.try_send(path.clone());
        }
    }

    fn apply_filter(&mut self) {
        let query = self.search_input.lines()[0].to_lowercase();

        self.filtered_wallpapers = self.wallpapers
            .iter()
            .filter(|p| p.file_name()
                .unwrap()
                .display()
                .to_string()
                .to_lowercase()
                .contains(&query))
            .cloned()
            .collect();
    }

    fn set_wallpaper(&mut self) -> io::Result<()> {
        if let Some(index) = self.wallpaper_list_state.selected() {
            let new_wall = &self.filtered_wallpapers[index].display().to_string();
            let command = match self.display_server {
                DisplayServer::X11 =>
                    Command::new("xwallpaper")
                        .args(["--stretch", new_wall])
                        .stderr(Stdio::piped())
                        .output(),
                DisplayServer::Wayland =>
                    Command::new("swaymsg")
                        .args(["output", "*", "bg", new_wall, "stretch"])
                        .stderr(Stdio::piped())
                        .output(),
                DisplayServer::Unknown => unreachable!()
            };

            match command {
                Ok(output) => {
                    if output.status.success() {
                        fs::write(&self.dot_wallpaper, new_wall)?;
                        self.message = format!("Set {} as a wallpaper", new_wall);
                        self.current_wallpaper = Some(new_wall.into());
                    } else {
                        let err_msg = String::from_utf8_lossy(&output.stderr);
                        self.message = format!("Failed to set {new_wall} as a wallpaper: {err_msg}");
                    }
                }
                Err(err) => self.message = format!("Failed to set {new_wall} as a wallpaper: {err}")
            }
        }

        Ok(())
    }

    fn run(&mut self, mut terminal: DefaultTerminal) -> io::Result<()> {
        loop {
            if let Ok(new_state) = self.image_rx.try_recv() {
                self.image_state = new_state;
            }

            terminal.draw(|frame| self.draw_ui(frame))?;

            if event::poll(Duration::from_millis(70))? && let Some(key) = event::read()?.as_key_press_event() {
                match self.mode {
                    Mode::Normal => match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            if self.search_input.is_empty() && self.indicator.is_none() {
                                break;
                            }
                            self.search_input.clear();
                            self.indicator = None;
                            self.apply_filter();
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            self.indicator = None;
                            self.wallpaper_list_state.select_next();
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            self.indicator = None;
                            self.wallpaper_list_state.select_previous();
                        }
                        KeyCode::Char('G') => {
                            self.indicator = None;
                            self.wallpaper_list_state.select_last();
                            self.shift_g_pressed = true;
                        }
                        KeyCode::Char('g') => {
                            if let Some('g') = self.indicator {
                                self.wallpaper_list_state.select_first();
                                self.indicator = None;
                            } else {
                                self.indicator = Some('g');
                            }
                        }
                        KeyCode::Char('o') => {
                            if let Some('g') = self.indicator {
                                self.wallpaper_list_state.select_first();
                            }
                            self.indicator = None;
                        }
                        KeyCode::Char('c') => {
                            self.indicator = None;
                            if let Some(current_wallpaper) = &self.current_wallpaper {
                                if let Some(index) = self.filtered_wallpapers.iter().position(|w| w == current_wallpaper) {
                                    if self.search_input.is_empty() {
                                        self.wallpaper_list_state.select(Some(index));
                                    }
                                    let current_wallpaper = self.wallpapers[index].display();
                                    self.message = format!("Current wallpaper is {current_wallpaper}");
                                }
                            } else {
                                self.message = String::from("No wallpaper is set");
                            }
                        }
                        KeyCode::Char('/') => {
                            self.wallpaper_list_state.select(None);
                            self.message.clear();
                            self.indicator = None;
                            self.mode = Mode::Search;
                        }
                        KeyCode::Enter => self.set_wallpaper()?,
                        _ => self.indicator = None,
                    }
                    Mode::Search => match key.code {
                        KeyCode::Esc => {
                            self.mode = Mode::Normal;
                            self.search_input.clear();
                            self.apply_filter();
                        }
                        KeyCode::Enter => self.mode = Mode::Normal,
                        _ => {
                            self.search_input.input(key);
                            self.apply_filter();
                        }
                    }
                }
            }

            if self.preview_update_timer.elapsed() >= Duration::from_millis(100) {
                self.preview_update_timer = Instant::now();
                self.update_selected_image(self.shift_g_pressed);
                self.shift_g_pressed = false;
            }
        }

        Ok(())
    }

    fn draw_ui(&mut self, frame: &mut Frame) {
        let [top_area, status_bar_area] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
        let [wallpaper_list_area, preview_area] = Layout::horizontal([Constraint::Max(30), Constraint::Fill(1)]).areas(top_area);

        self.draw_wallpaper_list(frame, wallpaper_list_area);
        self.draw_preview(frame, preview_area);
        self.draw_status_bar(frame, status_bar_area);
    }

    fn draw_wallpaper_list(&mut self, frame: &mut Frame, wallpaper_list_area: Rect) {
        if self.filtered_wallpapers.is_empty() {
            return;
        }

        let mut block = Block::bordered().title_alignment(HorizontalAlignment::Center);
 
        if let Some(index) = self.wallpaper_list_state.selected() {
            block = block.title(format!(" Wallpaper ({}/{}) ", index + 1, self.filtered_wallpapers.len()));
        } else {
            block = block.title(format!(" Wallpapers ({}) ", self.filtered_wallpapers.len()));
        }

        let wallpaper_names: Vec<ListItem> = self.filtered_wallpapers
            .iter()
            .map(|p| {
                let filename = p.file_name().unwrap().display().to_string();
                if let Some(current_wallpaper) = &self.current_wallpaper && let Some(index) = self.filtered_wallpapers.iter().position(|w| w == current_wallpaper) && p == &self.filtered_wallpapers[index] {
                    ListItem::new(format!("{filename} *")).style(Style::default().bg(Color::LightBlue).fg(Color::Black))
                } else {
                    ListItem::new(filename)
                }
            })
            .collect();

        let list = List::new(wallpaper_names)
            .style(Color::White)
            .block(block)
            .highlight_style(Style::default()
                .add_modifier(Modifier::REVERSED)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::ITALIC))
            .highlight_symbol("> ");

        frame.render_stateful_widget(list, wallpaper_list_area, &mut self.wallpaper_list_state);
    }

    fn draw_preview(&mut self, frame: &mut Frame, preview_area: Rect) {
        let preview_block = Block::bordered()
            .style(Color::White)
            .title(" Preview ")
            .title_alignment(HorizontalAlignment::Center);

        let inner_area = preview_block.inner(preview_area).centered(Constraint::Percentage(50), Constraint::Percentage(50));

        frame.render_widget(preview_block, preview_area);
        frame.render_stateful_widget(StatefulImage::new().resize(Resize::Scale(Some(FilterType::Lanczos3))), inner_area, &mut self.image_state);
    }

    fn draw_status_bar(&mut self, frame: &mut Frame, status_bar_area: Rect) {
        match self.mode {
            Mode::Normal => {
                let bar_layout = Layout::horizontal([Constraint::Percentage(90), Constraint::Percentage(10)]).split(status_bar_area);
                frame.render_widget(Line::from(self.message.as_str()).left_aligned(), bar_layout[0]);
                if let Some(c) = self.indicator {
                    frame.render_widget(Line::from(c.to_string()).left_aligned(), bar_layout[1]);
                }
            }
            Mode::Search => frame.render_widget(&self.search_input, status_bar_area)
        }
    }
}

fn main() -> io::Result<()> {
    let app_result = App::new()?.run(ratatui::init());
    ratatui::restore();
    app_result
}
