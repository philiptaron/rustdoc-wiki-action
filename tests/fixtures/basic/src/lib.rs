//! Crate docs for `basic`.
//!
//! # Overview
//!
//! Start with [`Config`], then look at [`util::helper`] and the [`Shape`] trait.
//! External items work too: [`Vec`] and [`std::collections::HashMap`].
//!
//! # Example
//!
//! ```
//! # use basic::Config;
//! # let hidden = 1;
//! let config = Config::new("demo");
//! ## not hidden
//! assert_eq!(config.name(), "demo");
//! ```

pub mod util;

/// Geometry.
pub mod shapes {
    /// Something with an area.
    pub trait Shape: std::fmt::Debug {
        /// The unit of measure.
        type Unit;

        /// Number of sides, if known.
        const SIDES: Option<u32> = None;

        /// Area of the shape.
        fn area(&self) -> f64;

        /// A provided method with a default body.
        fn describe(&self) -> String {
            format!("{self:?} with area {}", self.area())
        }
    }

    /// A circle.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Circle {
        /// Radius of the circle.
        pub radius: f64,
        hidden: u8,
    }

    impl Circle {
        /// Creates a circle. Links to [`Circle`] and [`Shape::area`].
        pub fn new(radius: f64) -> Self {
            Circle { radius, hidden: 0 }
        }
    }

    impl Shape for Circle {
        type Unit = f64;
        const SIDES: Option<u32> = Some(0);
        fn area(&self) -> f64 {
            std::f64::consts::PI * self.radius * self.radius
        }
    }
}

mod private_impl {
    /// Defined in a private module, re-exported below.
    pub struct Hidden {
        /// A public field.
        pub value: i32,
    }

    /// Also from the private module.
    pub fn inlined() {}
}

pub use private_impl::{inlined, Hidden as Reexported};
pub use shapes::*;
pub use std::collections::HashMap;

/// Runtime configuration.
///
/// Built with [`Config::new`]. See the [`Mode`] enum, [the util module](crate::util), and
/// [a reference-style link][helper].
///
/// # Errors
///
/// Never fails.
///
/// [helper]: crate::util::helper
#[derive(Debug, Default)]
#[must_use]
pub struct Config<T = ()> {
    name: String,
    /// User supplied extra data.
    pub extra: T,
    /// How to run.
    pub mode: Mode,
}

impl<T: Default> Config<T> {
    /// Creates a config with the given name.
    ///
    /// ```
    /// let c = basic::Config::<()>::new("x");
    /// ```
    pub fn new(name: &str) -> Self {
        Config { name: name.to_string(), extra: T::default(), mode: Mode::default() }
    }
}

impl<T> Config<T> {
    /// The name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Consumes the config and returns the extra data.
    pub const fn into_extra(self) -> T {
        self.extra
    }
}

/// How to run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mode {
    /// Do nothing.
    #[default]
    Idle,
    /// Run with a number of workers.
    Workers(usize),
    /// Run with named options.
    Custom {
        /// Verbosity level.
        level: u8,
        /// A label.
        label: String,
    },
}

/// Bits and pieces.
#[repr(C)]
pub union Bits {
    /// As an integer.
    pub int: u32,
    /// As a float.
    pub float: f32,
}

/// A unit struct.
pub struct Marker;

/// A tuple struct.
pub struct Pair<A, B>(pub A, pub B);

/// Generic function with bounds and a where clause.
///
/// Calls [`inlined`] and returns a [`Mode`].
pub fn convert<'a, T, U>(input: &'a T, f: impl Fn(&T) -> U) -> Option<U>
where
    T: Clone + Send + 'a,
    U: Into<String>,
{
    Some(f(input))
}

/// A constant.
pub const LIMIT: usize = 1 << 10;

/// A static.
pub static GREETING: &str = "hello";

/// A mutable static.
pub static mut COUNTER: u64 = 0;

/// A type alias.
pub type Table<V> = HashMap<String, V>;

/// Old and busted.
#[deprecated(since = "0.2.0", note = "use `convert` instead")]
pub fn old_convert() {}

/// An async, unsafe, extern function.
pub async unsafe fn ffi() {}

/// A C ABI function.
pub extern "C" fn c_abi(x: i32) -> i32 {
    x
}

/// Says hello.
///
/// ```
/// hello!();
/// ```
#[macro_export]
macro_rules! hello {
    () => {
        println!("hello")
    };
}
