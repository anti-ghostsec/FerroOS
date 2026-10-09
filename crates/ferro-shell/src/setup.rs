//! FerroOS Setup: the installation wizard, in the style of Windows 95's.
//! Welcome, choose a disk, confirm erasing it, copy, done. The privileged
//! work happens in ferro-system; this side only asks and shows progress.

use super::*;

const SU_BACK: usize = 0;
const SU_NEXT: usize = 1;
const SU_CANCEL: usize = 2;
const SU_CONFIRM: usize = 3;
const SU_DISK0: usize = 4;

/// A disk Setup may install to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskInfo {
    pub name: String,
    pub bytes: u64,
    pub model: String,
}

impl DiskInfo {
    /// `name|bytes|model;...` from ferro-system's `DISKS`.
    pub fn parse_list(s: &str) -> Vec<Self> {
        s.split(';')
            .filter_map(|d| {
                let mut f = d.split('|');
                Some(Self { name: f.next()?.to_owned(), bytes: f.next()?.parse().ok()?, model: f.next().unwrap_or("Disk").to_owned() })
            })
            .collect()
    }

    fn size_text(&self) -> String {
        let gb = self.bytes as f64 / 1e9;
        if gb >= 100.0 {
            format!("{gb:.0} GB")
        } else {
            format!("{gb:.1} GB")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Welcome,
    Disk,
    Confirm,
    Copying,
    Done,
    Failed,
}

pub(crate) struct SetupState {
    pub(crate) page: Page,
    disks: Vec<DiskInfo>,
    selected: Option<usize>,
    confirm: bool,
    error: Option<String>,
    percent: u32,
    step: String,
}

impl Shell {
    pub fn open_setup(&mut self) {
        if !self.focus_existing(|c| matches!(c, Content::Setup(_))) {
            let st =
                SetupState { page: Page::Welcome, disks: Vec::new(), selected: None, confirm: false, error: None, percent: 0, step: String::new() };
            self.open("FerroOS Setup".into(), icons::FERRO, (520, 360), Content::Setup(Box::new(st)));
        }
    }

    pub(crate) fn setup_items(&self, win: &Window, st: &SetupState) -> Vec<Rect> {
        let c = client_rect(win.rect);
        let by = c.bottom() - 34;
        let mut v = vec![
            Rect::new(c.right() - 262, by, 80, 23),
            Rect::new(c.right() - 182, by, 80, 23),
            Rect::new(c.right() - 90, by, 80, 23),
            Rect::new(c.x + 150, c.y + 150, c.w - 170, 16),
        ];
        if st.page == Page::Disk {
            v.extend((0..st.disks.len().min(6) as i32).map(|k| Rect::new(c.x + 152, c.y + 86 + k * 18, c.w - 174, 18)));
        }
        v
    }

    pub(crate) fn draw_setup(&self, s: &mut Surface, win: &Window, st: &SetupState, items: &[Rect]) {
        let c = client_rect(win.rect);
        // The wizard's picture panel.
        let panel = Rect::new(c.x + 8, c.y + 8, 128, c.h - 58);
        s.fill(panel, DESKTOP);
        s.bevel(panel, Bevel::Sunken);
        s.sprite(panel.x + 32, panel.y + 40, icons::FERRO, 4);
        s.etched_hline(c.x + 8, c.bottom() - 44, c.w - 16);

        let x = c.x + 150;
        let cols = ((c.w - 170) / 8) as usize;
        let para = |s: &mut Surface, y: i32, text: &str| -> i32 {
            let lines = wrap(text, cols);
            for (k, l) in lines.iter().enumerate() {
                s.text(x, y + k as i32 * 14, l, BLACK);
            }
            y + lines.len() as i32 * 14 + 10
        };
        let (title, back, next, next_on) = match st.page {
            Page::Welcome => ("Welcome to FerroOS Setup", false, "Next >", true),
            Page::Disk => ("Choose a disk", true, "Next >", st.selected.is_some()),
            Page::Confirm => ("Ready to install", true, "Install", st.confirm),
            Page::Copying => ("Installing FerroOS", false, "Next >", false),
            Page::Done => ("FerroOS is installed", false, "Restart", true),
            Page::Failed => ("Setup couldn't finish", true, "Next >", false),
        };
        s.text_bold(x, c.y + 16, title, BLACK);
        let y = c.y + 42;
        match st.page {
            Page::Welcome => {
                let y = para(s, y, "Setup puts FerroOS on this computer's disk, so it starts without the USB stick and can remember your settings and files, encrypted with a password you choose.");
                let y = para(s, y, "It takes about a minute. The disk you choose will be erased.");
                para(s, y, "Your computer must start in UEFI mode with Secure Boot turned off.");
            }
            Page::Disk => {
                para(s, y, "Where should FerroOS go?");
                let list = Rect::new(x, c.y + 84, c.w - 170, 6 * 18 + 4);
                s.fill(list, WHITE);
                s.bevel(list, Bevel::Sunken);
                if st.disks.is_empty() {
                    let msg = st.error.clone().unwrap_or_else(|| "No disk of 2 GB or more was found (the USB stick itself isn't offered).".into());
                    for (k, l) in wrap(&msg, cols - 2).iter().enumerate() {
                        s.text(list.x + 6, list.y + 8 + k as i32 * 14, l, SHADOW);
                    }
                }
                for (k, (d, r)) in st.disks.iter().zip(items.iter().skip(SU_DISK0)).enumerate() {
                    let on = st.selected == Some(k);
                    if on {
                        s.fill(*r, NAVY);
                    }
                    let fg = if on { WHITE } else { BLACK };
                    s.text(r.x + 4, r.y + 5, &ellipsize(&d.model, 22), fg);
                    s.text(r.x + 196, r.y + 5, &d.size_text(), fg);
                    s.text(r.x + 276, r.y + 5, &d.name, fg);
                }
            }
            Page::Confirm => {
                let d = st.selected.and_then(|i| st.disks.get(i));
                let what = d.map_or(String::new(), |d| format!("{} ({}, {})", d.model, d.size_text(), d.name));
                let y = para(s, y, &format!("Everything on {what} will be erased: every file, every partition, any other system."));
                para(s, y, "Nothing else on this computer is touched.");
                crate::privacy::draw_checkbox(s, items[SU_CONFIRM], st.confirm, "Erase this disk and install FerroOS");
            }
            Page::Copying => {
                let y = para(s, y, &format!("{}...", if st.step.is_empty() { "Starting" } else { &st.step }));
                // A Windows 95 progress bar: navy blocks in a sunken well.
                let bar = Rect::new(x, y + 6, c.w - 170, 20);
                s.fill(bar, WHITE);
                s.bevel(bar, Bevel::Sunken);
                let inner = bar.inset(3);
                let blocks = (inner.w / 10) * st.percent.min(100) as i32 / 100;
                for b in 0..blocks {
                    s.fill(Rect::new(inner.x + b * 10, inner.y, 8, inner.h), NAVY);
                }
            }
            Page::Done => {
                let y = para(s, y, "Remove the USB stick, then restart.");
                para(s, y, "When FerroOS starts from the disk, you'll choose the password that encrypts your settings and files.");
            }
            Page::Failed => {
                let y = para(s, y, st.error.as_deref().unwrap_or("Something went wrong."));
                para(s, y, "Go back to try again.");
            }
        }
        let held = |i: usize| self.is_held(Hit::Item(win.id, i), items[i]);
        for (i, label, enabled) in [(SU_BACK, "< Back", back), (SU_NEXT, next, next_on), (SU_CANCEL, "Cancel", st.page != Page::Copying)] {
            let r = items[i];
            s.button(r, enabled && held(i));
            let tx = r.x + (r.w - text_width(label)) / 2;
            if enabled {
                s.text(tx, r.y + 8, label, BLACK);
            } else {
                s.text_disabled(tx, r.y + 8, label);
            }
        }
    }

    fn setup_mut(&mut self, id: u32) -> Option<&mut SetupState> {
        match self.window_mut(id).map(|w| &mut w.content) {
            Some(Content::Setup(st)) => Some(st),
            _ => None,
        }
    }

    pub(crate) fn setup_click(&mut self, id: u32, item: usize) -> Option<Action> {
        let page = self.setup_mut(id)?.page;
        match (page, item) {
            (Page::Copying, _) => {}
            (_, SU_CANCEL) => self.close(id),
            (Page::Welcome, SU_NEXT) | (Page::Failed, SU_BACK) => {
                let listed = self.link.as_mut().map_or(Err("Setup isn't available here.".to_owned()), |l| l.list_disks());
                let st = self.setup_mut(id)?;
                st.page = Page::Disk;
                st.selected = None;
                match listed {
                    Ok(d) => {
                        st.disks = d;
                        st.error = None;
                    }
                    Err(e) => {
                        st.disks.clear();
                        st.error = Some(e);
                    }
                }
            }
            (Page::Disk, SU_BACK) => self.setup_mut(id)?.page = Page::Welcome,
            (Page::Disk, SU_NEXT) => {
                let st = self.setup_mut(id)?;
                if st.selected.is_some() {
                    st.page = Page::Confirm;
                    st.confirm = false;
                }
            }
            (Page::Disk, i) if i >= SU_DISK0 => self.setup_mut(id)?.selected = Some(i - SU_DISK0),
            (Page::Confirm, SU_BACK) => self.setup_mut(id)?.page = Page::Disk,
            (Page::Confirm, SU_CONFIRM) => {
                let st = self.setup_mut(id)?;
                st.confirm = !st.confirm;
            }
            (Page::Confirm, SU_NEXT) => {
                let st = self.setup_mut(id)?;
                let disk = st.selected.and_then(|i| st.disks.get(i)).filter(|_| st.confirm).map(|d| d.name.clone())?;
                let started = self.link.as_mut().map_or(Err("Setup isn't available here.".to_owned()), |l| l.install(&disk));
                let st = self.setup_mut(id)?;
                match started {
                    Ok(()) => {
                        st.page = Page::Copying;
                        st.percent = 0;
                        st.step.clear();
                    }
                    Err(e) => {
                        st.page = Page::Failed;
                        st.error = Some(e);
                    }
                }
            }
            (Page::Done, SU_NEXT) => return Some(Action::Reboot),
            _ => {}
        }
        None
    }

    /// Follows ferro-system's progress (`state=`, `percent=`, `step=`).
    pub(crate) fn setup_progress(&mut self, text: &str) {
        let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).unwrap_or("").to_owned();
        for w in &mut self.windows {
            let Content::Setup(st) = &mut w.content else { continue };
            if st.page != Page::Copying {
                continue;
            }
            st.percent = get("percent").parse().unwrap_or(st.percent);
            st.step = get("step");
            match get("state").as_str() {
                "done" => st.page = Page::Done,
                "error" => {
                    st.page = Page::Failed;
                    st.error = Some(st.step.clone());
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_disk_list() {
        let d = DiskInfo::parse_list("vda|8589934592|QEMU HARDDISK;nvme0n1|512110190592|Samsung SSD");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].size_text(), "8.6 GB");
        assert_eq!(d[1].size_text(), "512 GB");
        assert!(DiskInfo::parse_list("").is_empty());
    }
}
