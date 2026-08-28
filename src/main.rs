#![allow(clippy::collapsible_if)]

use std::{
    env,
    io,
    time::{Duration, Instant},
    path::PathBuf, fs,
    process::{Command, Stdio},
    thread, sync::{Arc, mpsc::{self, SyncSender, Receiver}},
    rc::Rc,
    collections::HashMap
};
use ratatui::{
    crossterm::event::{self, KeyCode, Event},
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

fn spawn_image_decoder(picker: Arc<Picker>, image_tx: SyncSender<StatefulProtocol>, path_rx: Receiver<PathBuf>) {
    thread::spawn(move || {
        let mut cache: HashMap<PathBuf, Rc<DynamicImage>> = HashMap::new();

        while let Ok(path) = path_rx.recv() {
            let preview = {
                if let Some(cached_image) = cache.get(&path) {
                    cached_image.clone()
                } else {
                    match image::ImageReader::open(&path).and_then(|r| r.decode().map_err(io::Error::other)) {
                        Ok(image) => {
                            let thumbnail = Rc::new(image.thumbnail(800, 600));
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
}

fn get_wallpapers_from(path: PathBuf) -> Vec<PathBuf> {
    let exts = ["jpg", "jpeg", "png", "gif", "webp"];
    WalkDir::new(path) 
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|path| {
            path.is_file() && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| exts.contains(&ext.to_lowercase().as_str()))
        })
        .collect()
}

fn collect_wallpapers() -> Vec<PathBuf> {
    let mut wallpapers = Vec::new();

    let home = std::env::home_dir().unwrap();

    wallpapers.extend(get_wallpapers_from("/usr/share/backgrounds".into()));
    wallpapers.extend(get_wallpapers_from(home.join(".local/share/backgrounds")));
    wallpapers.sort_unstable();

    wallpapers
}

fn load_current_wallpaper(config: &PathBuf, wallpapers: &[PathBuf], wallpaper_list_state: &mut ListState, path_tx: &SyncSender<PathBuf>) -> Option<PathBuf> {
    if let Ok(string) = fs::read_to_string(config) {
        let wallpaper = PathBuf::from(string);
        if let Some(index) = wallpapers.iter().position(|w| w == &wallpaper) {
            *wallpaper_list_state = wallpaper_list_state.with_selected(Some(index));
            let _ = path_tx.try_send(wallpaper.clone());
            return Some(wallpaper);
        }
    }

    None
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
    config: PathBuf,
    indicator: Option<char>,
    message: String,
    mode: Mode,
    search_input: TextArea<'static>,
    current_wallpaper: Option<PathBuf>,
    path_tx: SyncSender<PathBuf>,
    image_rx: Receiver<StatefulProtocol>,
    preview_update_timer: Instant,
    shift_g_pressed: bool,
    display_server: DisplayServer,
    last_previewed: Option<PathBuf>
}

impl App {
    fn new() -> io::Result<Self> {
        let display_server = detect_display_server();
        if display_server == DisplayServer::Unknown {
            return Err(io::Error::other("Unknown display server"));
        }

        let (path_tx, path_rx) = mpsc::sync_channel::<PathBuf>(1);
        let (image_tx, image_rx) = mpsc::sync_channel::<StatefulProtocol>(1);

        let picker = Picker::from_query_stdio().map_err(io::Error::other)?;
        let image_state = picker.new_resize_protocol(DynamicImage::default());

        spawn_image_decoder(Arc::new(picker), image_tx, path_rx);

        let wallpapers = collect_wallpapers();

        let mut wallpaper_list_state = ListState::default().with_selected(Some(0));

        let config = PathBuf::from(format!("{}/.twall", env::home_dir().unwrap().display()));
        let current_wallpaper = load_current_wallpaper(&config, &wallpapers, &mut wallpaper_list_state, &path_tx);

        let mut search_input = TextArea::default();
        search_input.set_cursor_line_style(Style::default().white());
        search_input.set_placeholder_text("Search wallpaper...");

        Ok(Self {
            wallpaper_list_state,
            wallpapers: wallpapers.clone(),
            filtered_wallpapers: wallpapers,
            image_state,
            config,
            indicator: None,
            message: String::new(),
            mode: Mode::Normal,
            search_input,
            current_wallpaper,
            path_tx,
            image_rx,
            preview_update_timer: Instant::now(),
            shift_g_pressed: false,
            display_server,
            last_previewed: None
        })
    }

    fn update_selected_image(&mut self, last: bool) {
        let path = if last {
            self.filtered_wallpapers.last().cloned()
        } else {
            self.wallpaper_list_state.selected().and_then(|i| self.filtered_wallpapers.get(i).cloned())
        };

        if let Some(path) = path {
            if self.last_previewed.as_ref() != Some(&path) {
                if self.path_tx.try_send(path.clone()).is_ok() {
                    self.last_previewed = Some(path);
                }
            }
        }
    }

    fn apply_filter(&mut self) {
        let query = self.search_input.lines()[0].trim().to_lowercase();

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

        self.last_previewed = None;
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
                        fs::write(&self.config, new_wall)?;
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
        let mut need_redraw = true;

        loop {
            if let Ok(new_state) = self.image_rx.try_recv() {
                self.image_state = new_state;
                need_redraw = true;
            }

            if need_redraw {
                terminal.draw(|frame| self.draw_ui(frame))?;
                need_redraw = false;
            }

            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Resize(_, _) => need_redraw = true,
                    Event::Key(key) => {
                        need_redraw = true;
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
                                    if self.wallpaper_list_state.selected().is_none_or(|index| index != self.filtered_wallpapers.len().saturating_sub(1)) {
                                        self.wallpaper_list_state.select_next();
                                    }
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    self.indicator = None;
                                    self.wallpaper_list_state.select_previous();
                                }
                                KeyCode::Char('G') => {
                                    self.indicator = None;
                                    if self.wallpaper_list_state.selected().is_none_or(|index| index != self.filtered_wallpapers.len().saturating_sub(1)) {
                                        self.wallpaper_list_state.select_last();
                                    }
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
                                            self.message = format!("Current wallpaper is {}", current_wallpaper.display());
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
                    _ => {}
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
                if let Some(current_wallpaper) = &self.current_wallpaper && self.filtered_wallpapers.contains(current_wallpaper) && p == current_wallpaper {
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
        frame.render_stateful_widget(StatefulImage::new().resize(Resize::Scale(Some(FilterType::Nearest))), inner_area, &mut self.image_state);
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
