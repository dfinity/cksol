use cksol_types_internal::event::{Event, EventType};

pub struct MinterEventAssert {
    events: Vec<EventType>,
}

impl MinterEventAssert {
    pub fn new<T: IntoIterator<Item = Event>>(events: T) -> Self {
        let events: Vec<_> = events.into_iter().map(|e| e.payload).collect();
        Self { events }
    }

    pub fn satisfy<F>(self, check: F) -> Self
    where
        F: Fn(&[EventType]),
    {
        check(&self.events);
        self
    }
}
