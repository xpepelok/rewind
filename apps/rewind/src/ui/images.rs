use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use futures::FutureExt;
use futures::future::Shared;
use gpui::{
    App, AppContext, Asset, AssetLogger, Entity, ImageAssetLoader, ImageCache, ImageCacheError,
    RenderImage, Resource, Task, Window, hash,
};

type Loaded = Result<Arc<RenderImage>, ImageCacheError>;

enum Slot {
    Loading(Shared<Task<Loaded>>),
    Ready(Loaded),
}

struct Entry {
    slot: Slot,
    used: u64,
}

impl Entry {
    fn image(self) -> Option<Arc<RenderImage>> {
        match self.slot {
            Slot::Ready(loaded) => loaded.ok(),
            Slot::Loading(task) => task.now_or_never().and_then(Result::ok),
        }
    }

    fn ready(&self) -> bool {
        matches!(self.slot, Slot::Ready(_))
    }
}

pub struct Images {
    entries: HashMap<u64, Entry>,
    clock: u64,
    capacity: usize,
}

impl Images {
    pub fn new(capacity: usize, cx: &mut App) -> Entity<Self> {
        let images = cx.new(|_| Self {
            entries: HashMap::new(),
            clock: 0,
            capacity,
        });
        cx.observe_release(&images, |images, cx| {
            for (_, entry) in images.entries.drain() {
                if let Some(image) = entry.image() {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();
        images
    }

    pub fn forget(&mut self, path: &Path, window: &mut Window, cx: &mut App) {
        let key = hash(&Resource::Path(Arc::from(path)));
        if let Some(image) = self.entries.remove(&key).and_then(Entry::image) {
            cx.drop_image(image, Some(window));
        }
    }

    pub fn trim(&mut self, keep: usize, window: &mut Window, cx: &mut App) {
        while self.entries.len() > keep {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.ready())
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| *key);
            let Some(oldest) = oldest else {
                break;
            };
            if let Some(image) = self.entries.remove(&oldest).and_then(Entry::image) {
                cx.drop_image(image, Some(window));
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

impl ImageCache for Images {
    fn load(&mut self, resource: &Resource, window: &mut Window, cx: &mut App) -> Option<Loaded> {
        self.clock += 1;
        let key = hash(resource);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.clock;
            let loaded = match &mut entry.slot {
                Slot::Ready(loaded) => return Some(loaded.clone()),
                Slot::Loading(task) => task.now_or_never()?,
            };
            entry.slot = Slot::Ready(loaded.clone());
            return Some(loaded);
        }
        let load = AssetLogger::<ImageAssetLoader>::load(resource.clone(), cx);
        let task = cx.background_executor().spawn(load).shared();
        self.entries.insert(
            key,
            Entry {
                slot: Slot::Loading(task.clone()),
                used: self.clock,
            },
        );
        let view = window.current_view();
        window
            .spawn(cx, async move |cx| {
                task.await.ok();
                cx.on_next_frame(move |_, cx| cx.notify(view));
            })
            .detach();
        let capacity = self.capacity;
        self.trim(capacity, window, cx);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn old_images_make_room_for_new_ones(cx: &mut gpui::TestAppContext) {
        let images = cx.update(|cx| Images::new(2, cx));
        images.update(cx, |images, _| {
            for (index, used) in [3, 1, 2].into_iter().enumerate() {
                images.entries.insert(
                    index as u64,
                    Entry {
                        slot: Slot::Ready(Err(ImageCacheError::Asset("gone".into()))),
                        used,
                    },
                );
            }
        });
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            images.update(cx, |images, cx| images.trim(2, window, cx));
        });
        images.update(cx, |images, _| {
            assert_eq!(images.len(), 2);
            assert!(!images.entries.contains_key(&1));
        });
    }
}
