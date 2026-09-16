use std::sync::Arc;

use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

pub trait MergeSelf: Default + MergeBy<Self> {}

#[async_trait]
pub trait MergeEffectiveConfig<E> {
    async fn get_effective_config(&self) -> E;
}

pub trait MergeBy<C>: Serialize + DeserializeOwned {
    fn merge(&mut self, other: &C);
}

pub trait ArcField<T> {
    fn as_ref(&self) -> &T;
    fn as_mut(&mut self) -> &mut T;
}

pub trait OptionArcField<T> {
    fn as_deref(&self) -> Option<&T>;
    fn as_deref_mut(&mut self) -> Option<&mut T>;
    fn insert(&mut self, value: Arc<T>);
    fn replace(&mut self, value: Arc<T>) -> Option<Arc<T>>;
    fn remove(&mut self);
    fn take(&mut self) -> Option<Arc<T>>;
    fn get(&self) -> Option<Arc<T>>;
}

#[macro_export]
macro_rules! impl_arc_field {
    ($ty:ty => $field:ident, $owner:ty) => {
        impl ArcField<$ty> for $owner {
            fn as_ref(&self) -> &$ty {
                self.$field.as_ref()
            }
            fn as_mut(&mut self) -> &mut $ty {
                Arc::make_mut(&mut self.$field)
            }
        }
    };
}

#[macro_export]
macro_rules! impl_option_arc_field {
    ($ty:ty => $field:ident, $owner:ty) => {
        impl OptionArcField<$ty> for $owner {
            fn as_deref(&self) -> Option<&$ty> {
                self.$field.as_deref()
            }
            fn as_deref_mut(&mut self) -> Option<&mut $ty> {
                self.$field.as_mut().map(|arc| Arc::make_mut(arc))
            }
            fn insert(&mut self, value: Arc<$ty>) {
                self.$field = Some(value);
            }
            fn replace(&mut self, value: Arc<$ty>) -> Option<Arc<$ty>> {
                self.$field.replace(value)
            }
            fn remove(&mut self) {
                self.$field = None;
            }
            fn take(&mut self) -> Option<Arc<$ty>> {
                self.$field.take()
            }
            fn get(&self) -> Option<Arc<$ty>> {
                self.$field.clone()
            }
        }
    };
}

#[macro_export]
macro_rules! impl_arc_field_map {
    ($map:ty) => {
        impl $map {
            fn get<T>(&self) -> &T
            where
                Self: ArcField<T>,
            {
                <Self as ArcField<T>>::as_ref(self)
            }

            fn get_mut<T>(&mut self) -> Option<&mut T>
            where
                Self: ArcField<T>,
            {
                <Self as ArcField<T>>::as_mut(self)
            }
        }
    };
}

#[macro_export]
macro_rules! impl_option_arc_field_map {
    ($map:ty) => {
        impl $map {
            pub fn get_deref<T>(&self) -> Option<&T>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::as_deref(self)
            }

            pub fn get_deref_mut<T>(&mut self) -> Option<&mut T>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::as_deref_mut(self)
            }

            pub fn insert<T>(&mut self, value: Arc<T>)
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::insert(self, value)
            }

            pub fn replace<T>(&mut self, value: Arc<T>) -> Option<Arc<T>>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::replace(self, value)
            }

            pub fn remove<T>(&mut self)
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::remove(self)
            }

            pub fn take<T>(&mut self) -> Option<Arc<T>>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::take(self)
            }

            pub fn get<T>(&self) -> Option<Arc<T>>
            where
                Self: OptionArcField<T>,
            {
                <Self as OptionArcField<T>>::get(self)
            }
        }
    };
}
