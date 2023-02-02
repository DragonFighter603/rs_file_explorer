use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{stdout, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::{env, thread};
use getch::Getch;
use crate::files::{collect_items, Item, ItemType};
use crate::os_generic::{config_dir, enable_virtual_terminal_processing, fmt_canonical_path, MetadataExt};

pub(crate) fn quit() -> ! {
    #[cfg(windows)]
    File::create(&format!("{}/cc_cwd", config_dir())).unwrap().write_all(&env::current_dir().unwrap().to_str().unwrap().as_bytes()[4..]).unwrap();
    #[cfg(not(windows))]
    File::create(&format!("{}/cc_cwd", config_dir())).unwrap().write_all(env::current_dir().unwrap().to_str().unwrap().as_bytes()).unwrap();
    print!("\x1b[?47l");  // restore screen
    print!("\x1b[?25h");  // show cursor
    let _ = stdout().flush();
    exit(0)
}

pub(crate) fn truncate_str(str: &str, len: usize) -> String{
    if str.len() < len - 2 {
        str.to_string()
    } else {
        str.split_at(len - 3).0.to_string() + "..."
    }
}

pub(crate) struct Cli {
    input: Arc<Mutex<VecDeque<u8>>>,
    path: PathBuf,
    dir_items: Vec<Item>,
    sorted_items: Vec<Item>,
    selected_item: usize,
    query_string: String
}

impl Cli {
    pub(crate) fn start(dir: &str) {
        let path = Path::new(dir).canonicalize().unwrap();
        enable_virtual_terminal_processing();
        print!("\x1b[?25l");  // hide cursor
        print!("\x1b[?47h");  // save screen
        print!("\x1b[2J");    // erase screen
        let _ = stdout().flush();
        let mut cli = Self {
            input: Default::default(),
            path,
            selected_item: 0,
            dir_items: vec![],
            sorted_items: vec![],
            query_string: String::new()
        };
        cli.move_dir(".");
        cli.resort();
        let thread_in = cli.input.clone();
        thread::spawn(move ||{
            let getch = Getch::new();
            loop {
                let ch = getch.getch().unwrap();
                thread_in.lock().unwrap().push_back(ch);
            }
        });
        cli.run();
        ctrlc::set_handler(|| quit()).expect("Error setting Ctrl-C handler");
    }

    fn run(&mut self){
        loop {
            if let Some(c) = self.input() {
                self.handle_input(c)
            }
            // move to top left, print
            print!("\x1b[H{}", self.render())
        }
    }

    fn handle_input(&mut self, c: u8){
        match c {
            3 => quit(),  // ctrl-c
            224 => match self.await_input() {
                72 => if self.selected_item > 0 { self.selected_item -= 1 },  // up
                80 =>  if self.selected_item < self.sorted_items.len() - 1 { self.selected_item += 1 },  // down
                75 => (),  // left
                77 => (),  // right

                71 => (),  // start
                79 => (),  // end
                73 => self.selected_item = 0,  // screen up
                81 => self.selected_item -= 1, // screen down

                83 => self.move_dir(".."), // remove
                _ => ()
            }
            27 => self.move_dir(".."),  // delete | esc
            b'\r' => {
                if self.selected_item < self.dir_items.len() {
                    let item = &self.sorted_items[self.selected_item];
                    match item.ty {
                        ItemType::File(_) | ItemType::Link(_, true) => {
                            let _ = open::that(self.path.join(&item.name));
                        }
                        ItemType::Dir if item.name == "." => {
                            let _ = open::that(self.path.join(&item.name));
                        }
                        ItemType::Dir | ItemType::Link(_, false) => {
                            self.move_dir(&item.name.clone());
                        }
                    }
                }
            }
            8 => {  // backspace
                let _ = self.query_string.pop();
                self.resort();
            }
            b' '..=b'~' => {
                self.query_string.push(c as char);
                self.resort();
            },
            _ => ()
        }
    }

    fn render(&self) -> String {
        let (w, h) = Self::size();
        let mut out = String::new();
        out.push_str(&format!("{:1$}", fmt_canonical_path(&self.path), w));
        for (i, item) in self.sorted_items.iter().enumerate() {
            if i == self.selected_item {
                out.push_str(&format!("> {:1$}", item.render(), w - 2));
            } else {
                out.push_str(&format!("  {:1$}", item.render(), w - 2));
            }
        }
        for _ in self.sorted_items.len()..h-2 {
            out.push_str(&format!("{:1$}", "", w));
        }
        out.push_str(&format!(":{:1$}", self.query_string.clone() + "¦", w - 1));
        out
    }

    fn resort(&mut self) {
        let query = &self.query_string.to_ascii_lowercase();
        let mut sorted = self.dir_items.clone();
        sorted.sort_by(|a, b| {
            let cased_idx_a = a.name.find(&self.query_string);
            let cased_idx_b = b.name.find(&self.query_string);
            match (cased_idx_a, cased_idx_b) {
                (Some(_), None) => return Ordering::Less,
                (None, Some(_)) => return Ordering::Greater,
                _ => ()
            }
            let name_a = a.name.to_ascii_lowercase();
            let name_b = b.name.to_ascii_lowercase();
            let idx_a = name_a.find(query);
            let idx_b = name_b.find(query);
            match (idx_a, idx_b) {
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(i_a), Some(i_b)) if i_a != i_b => i_a.cmp(&i_b),
                _ => name_a.cmp(&name_b)
            }
        });
        let self_meta = self.path.metadata().unwrap();
        sorted.insert(0, Item {
            ty: ItemType::Dir,
            name: ".".to_string(),
            readonly: self_meta.permissions().readonly(),
            created: self_meta.creation_time(),
            last_used: self_meta.last_access_time(),
            last_written: self_meta.last_write_time(),
        });
        let parent_meta = self.path.join("..").metadata().unwrap();
        sorted.insert(0, Item{
            ty: ItemType::Dir,
            name: "..".to_string(),
            readonly: parent_meta.permissions().readonly(),
            created: parent_meta.creation_time(),
            last_used: parent_meta.last_access_time(),
            last_written: parent_meta.last_write_time(),
        });
        self.sorted_items = sorted;
    }
    fn move_dir(&mut self, rel: &str) {
        if let Ok(path) = self.path.join(rel).canonicalize() {
            self.path = path;
            self.dir_items = collect_items(self.path.to_str().unwrap());
            self.query_string = String::new();
            env::set_current_dir(self.path.clone()).expect("unable to set cwd");
            self.resort();
            self.selected_item = self.sorted_items.len().min(2);
        }
    }

    fn input(&self) -> Option<u8>{
        self.input.lock().unwrap().pop_front()
    }

    fn await_input(&self) -> u8{
        loop {
            if let Some(c) = self.input.lock().unwrap().pop_front() {
                return c
            }
        }
    }

    fn size() -> (usize, usize) {
        let (w, h) = term_size::dimensions().expect("error getting size of terminal");
        (w, h)
    }
}
