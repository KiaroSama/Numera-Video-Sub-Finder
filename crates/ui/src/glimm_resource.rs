use std::ops::Deref;

pub(crate) struct OwnedResource<T> {
    value: T,
    release: Box<dyn FnMut(&T)>,
}

impl<T> OwnedResource<T> {
    pub(crate) fn new(value: T, release: impl FnMut(&T) + 'static) -> Self {
        Self {
            value,
            release: Box::new(release),
        }
    }
}

impl<T> Deref for OwnedResource<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> Drop for OwnedResource<T> {
    fn drop(&mut self) {
        (self.release)(&self.value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn failed_initialization_releases_every_created_resource_once() {
        let released = Rc::new(RefCell::new(Vec::new()));
        for fail_at in 0..4 {
            let result: Option<()> = (|| {
                let mut resources = Vec::new();
                for stage in 0..4 {
                    if stage == fail_at {
                        return None;
                    }
                    let released = released.clone();
                    resources.push(OwnedResource::new(stage, move |id| {
                        released.borrow_mut().push(*id);
                    }));
                }
                Some(())
            })();
            assert!(result.is_none());
            let mut actual = released.replace(Vec::new());
            actual.sort_unstable();
            assert_eq!(actual, (0..fail_at).collect::<Vec<_>>());
        }
    }

    #[test]
    fn shared_successful_resource_stays_owned_until_last_user_releases_it() {
        let released = Rc::new(RefCell::new(Vec::new()));
        let record = released.clone();
        let resource = Rc::new(OwnedResource::new("canvas", move |value| {
            record.borrow_mut().push(*value);
        }));
        let frame = resource.clone();
        assert_eq!(&**resource, &"canvas");
        drop(resource);
        assert!(released.borrow().is_empty());
        drop(frame);
        assert_eq!(&*released.borrow(), &["canvas"]);
    }
}
