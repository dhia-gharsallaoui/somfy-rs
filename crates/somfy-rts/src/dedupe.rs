use crate::Frame;
use heapless::FnvIndexMap;

/// Number of distinct `(address, rolling_code)` pairs tracked at once. RTS
/// remotes only ever have a handful of presses in flight within a dedupe
/// window, so a small bounded map is plenty; it MUST stay a power of two for
/// [`FnvIndexMap`].
const CAPACITY: usize = 8;

/// Collapses RTS repeat frames (1 first frame + N repeats per button press)
/// into a single logical event per `(address, rolling_code)` within a time
/// window.
///
/// A remote transmits the same frame several times for one press so the
/// receiver has multiple chances to hear it. The domain layer wants exactly one
/// event per press, so [`accept`](RxDeduper::accept) returns `true` only for the
/// first occurrence of a given pair inside `window_ms`; later repeats within the
/// window return `false`. Once the window elapses the pair is treated as a fresh
/// event again (a remote reusing a rolling code after that long is a new press).
///
/// The tracking map is bounded to [`CAPACITY`] entries; when it is full and a
/// new pair arrives, the oldest entry is evicted to make room.
pub struct RxDeduper {
    window_ms: u32,
    seen: FnvIndexMap<(u32, u16), u32, CAPACITY>,
}

impl RxDeduper {
    pub fn new(window_ms: u32) -> Self {
        RxDeduper {
            window_ms,
            seen: FnvIndexMap::new(),
        }
    }

    /// Returns `true` if `frame` is the first occurrence of its
    /// `(address, rolling_code)` pair within `window_ms` of the last one, and
    /// `false` if it is a repeat that should be suppressed.
    ///
    /// `now_ms` is a monotonic millisecond clock; subtraction is wrapping so the
    /// deduper stays correct across the `u32` rollover (~49.7 days).
    pub fn accept(&mut self, frame: &Frame, now_ms: u32) -> bool {
        let key = (frame.address, frame.rolling_code);
        if let Some(&t) = self.seen.get(&key) {
            if now_ms.wrapping_sub(t) < self.window_ms {
                return false;
            }
        }
        if self.seen.len() == self.seen.capacity() && !self.seen.contains_key(&key) {
            self.evict_oldest(now_ms);
        }
        let _ = self.seen.insert(key, now_ms);
        true
    }

    /// Evict the entry with the greatest age (`now_ms - t`), i.e. the one seen
    /// longest ago. Using `max_by_key` on the wrapping age directly is both the
    /// simplest and the correct choice: an entry inserted at exactly `now_ms`
    /// has age 0 (the newest) and is therefore never selected.
    fn evict_oldest(&mut self, now_ms: u32) {
        if let Some(oldest) = self
            .seen
            .iter()
            .max_by_key(|(_, &t)| now_ms.wrapping_sub(t))
            .map(|(k, _)| *k)
        {
            self.seen.remove(&oldest);
        }
    }
}
