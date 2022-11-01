/// Creates an struct where the members can be used as integers, similar to C enums.
///
/// # Examples
///
/// ```
/// int_enum! {
///     /// My enum
///     pub struct Test : u8 {
///        const VAL_1 = 0x0;
///        const VAL_2 = 0x1;
///     }
/// }
/// ```
///
/// Each struct member has the field `val`, which corresponds to its value. The macro implements the
/// traits [`Debug`](core::fmt::Debug), [`Display`](core::fmt::Display),
/// [`Marshallable`](crate::serialize::Marshallable), and
/// [`Unmarshallable`](crate::serialize::Unmarshallable). Furthermore, it allows to convert from the
/// underlying type (here [`u8`]) to the struct.
#[macro_export]
macro_rules! int_enum {
    (
        $(#[$outer:meta])*
        pub struct $Name:ident: $T:ty {
            $(
                $(#[$inner:ident $($args:tt)*])*
                const $Flag:ident = $value:expr;
            )+
        }
    ) => (
        $(#[$outer])*
        #[derive(Copy, PartialEq, Eq, Clone, PartialOrd, Ord)]
        pub struct $Name {
            pub val: $T,
        }

        int_enum! {
            @enum_impl struct $Name : $T {
                $(
                    $(#[$inner $($args)*])*
                    const $Flag = $value;
                )+
            }
        }
    );

    (
        $(#[$outer:meta])*
        struct $Name:ident: $T:ty {
            $(
                $(#[$inner:ident $($args:tt)*])*
                const $Flag:ident = $value:expr;
            )+
        }
    ) => (
        $(#[$outer])*
        #[derive(Copy, PartialEq, Eq, Clone, PartialOrd, Ord)]
        struct $Name {
            pub val: $T,
        }

        int_enum! {
            @enum_impl struct $Name : $T {
                $(
                    const $Flag = $value;
                )+
            }
        }
    );

    (
        @enum_impl struct $Name:ident: $T:ty {
            $(
                $(#[$attr:ident $($args:tt)*])*
                const $Flag:ident = $value:expr;
            )+
        }
    ) => (
        impl $Name {
            $(
                $(#[$attr $($args)*])*
                #[allow(dead_code)]
                pub const $Flag: $Name = $Name { val: $value };
            )+
        }

        impl From<$T> for $Name {
            fn from(val: $T) -> Self {
                $Name { val }
            }
        }
    )
}
