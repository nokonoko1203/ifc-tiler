//! A thin RAII wrapper around PROJ (proj-sys). `unsafe` and raw pointers are confined to this file.
//!
//! The context (`Context`) and objects (`Object`) are destroyed in `Drop`.
//! Objects hold a reference count of the context and are destroyed before it.

use std::ffi::{CStr, CString};
use std::ptr::{self, NonNull};
use std::rc::Rc;

use proj_sys as sys;

/// Kind of CRS. Only the kinds used for decisions are distinguished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Compound,
    Projected,
    Geographic2d,
    Geographic3d,
    Other,
}

struct RawContext(NonNull<sys::PJ_CONTEXT>);

impl Drop for RawContext {
    fn drop(&mut self) {
        // SAFETY: destroys the one context that was created exactly once. Objects hold it and have already been destroyed
        unsafe { sys::proj_context_destroy(self.0.as_ptr()) };
    }
}

/// A PROJ context. Clones share the same context.
#[derive(Clone)]
pub struct Context(Rc<RawContext>);

impl Context {
    /// `None` if it cannot be created.
    pub fn new() -> Option<Self> {
        // SAFETY: takes no arguments. The returned NULL is checked
        let ctx = NonNull::new(unsafe { sys::proj_context_create() })?;
        Some(Self(Rc::new(RawContext(ctx))))
    }

    fn raw(&self) -> *mut sys::PJ_CONTEXT {
        self.0.0.as_ptr()
    }

    /// Enables or disables downloading grids over the network.
    pub fn set_network(&self, enable: bool) {
        // SAFETY: ctx is valid
        unsafe { sys::proj_context_set_enable_network(self.raw(), i32::from(enable)) };
    }

    /// Enables the grid cache.
    pub fn enable_grid_cache(&self) {
        // SAFETY: ctx is valid
        unsafe { sys::proj_grid_cache_set_enable(self.raw(), 1) };
    }

    fn wrap(&self, pj: *mut sys::PJ) -> Option<Object> {
        NonNull::new(pj).map(|pj| Object { pj, ctx: self.clone() })
    }

    /// Creates an object from a definition string (`EPSG:4326`, WKT, …). `None` if it cannot be parsed.
    pub fn create(&self, definition: &str) -> Option<Object> {
        let def = CString::new(definition).ok()?;
        // SAFETY: ctx is valid. def is NUL-terminated
        self.wrap(unsafe { sys::proj_create(self.raw(), def.as_ptr()) })
    }

    /// Creates a compound CRS from a horizontal CRS and a vertical CRS.
    pub fn compound_crs(&self, name: &str, horizontal: &Object, vertical: &Object) -> Option<Object> {
        let name = CString::new(name).ok()?;
        // SAFETY: ctx and both objects are valid (objects hold ctx)
        self.wrap(unsafe {
            sys::proj_create_compound_crs(self.raw(), name.as_ptr(), horizontal.pj.as_ptr(), vertical.pj.as_ptr())
        })
    }

    /// Creates a transformation from `source` to `target` (PROJ picks from the candidates).
    pub fn crs_to_crs(&self, source: &Object, target: &Object) -> Option<Object> {
        // SAFETY: ctx and both objects are valid. No area or options are specified
        self.wrap(unsafe {
            sys::proj_create_crs_to_crs_from_pj(
                self.raw(),
                source.pj.as_ptr(),
                target.pj.as_ptr(),
                ptr::null_mut(),
                ptr::null(),
            )
        })
    }
}

/// A PROJ object (a CRS or a transformation). Destroyed exactly once in `Drop`.
pub struct Object {
    pj: NonNull<sys::PJ>,
    ctx: Context,
}

impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: destroys the one object that was created exactly once. ctx is destroyed afterwards
        unsafe { sys::proj_destroy(self.pj.as_ptr()) };
    }
}

impl Object {
    /// Kind of CRS.
    pub fn kind(&self) -> Kind {
        // SAFETY: pj is valid
        match unsafe { sys::proj_get_type(self.pj.as_ptr()) } {
            sys::PJ_TYPE_PJ_TYPE_COMPOUND_CRS => Kind::Compound,
            sys::PJ_TYPE_PJ_TYPE_PROJECTED_CRS => Kind::Projected,
            sys::PJ_TYPE_PJ_TYPE_GEOGRAPHIC_2D_CRS => Kind::Geographic2d,
            sys::PJ_TYPE_PJ_TYPE_GEOGRAPHIC_3D_CRS => Kind::Geographic3d,
            _ => Kind::Other,
        }
    }

    /// The `index`-th sub-CRS of a compound CRS.
    pub fn sub_crs(&self, index: i32) -> Option<Object> {
        // SAFETY: ctx and pj are valid
        self.ctx.wrap(unsafe { sys::proj_crs_get_sub_crs(self.ctx.raw(), self.pj.as_ptr(), index) })
    }

    /// The transformation with axis order normalized to (east, north) or (longitude, latitude).
    pub fn normalize_for_visualization(&self) -> Option<Object> {
        // SAFETY: ctx and pj are valid
        self.ctx.wrap(unsafe { sys::proj_normalize_for_visualization(self.ctx.raw(), self.pj.as_ptr()) })
    }

    /// Transforms one point in the forward direction. `None` if the result is not finite.
    pub fn trans(&self, [x, y, z]: [f64; 3]) -> Option<[f64; 3]> {
        // SAFETY: pj is a valid transformation
        let [x, y, z, _] =
            unsafe { sys::proj_trans(self.pj.as_ptr(), sys::PJ_DIRECTION_PJ_FWD, sys::proj_coord(x, y, z, 0.0)).v };
        [x, y, z].iter().all(|v| v.is_finite()).then_some([x, y, z])
    }

    /// The operation used by the last transformation. `None` for a transformation with a single candidate (the transformation itself is the operation).
    pub fn last_used_operation(&self) -> Option<Object> {
        // SAFETY: pj is valid. The return value is a copy, so Object destroys it
        self.ctx.wrap(unsafe { sys::proj_trans_get_last_used_operation(self.pj.as_ptr()) })
    }

    /// Whether the operation includes an approximation that does not use grids (ballpark).
    pub fn has_ballpark_transformation(&self) -> bool {
        // SAFETY: ctx and pj are valid
        unsafe { sys::proj_coordoperation_has_ballpark_transformation(self.ctx.raw(), self.pj.as_ptr()) == 1 }
    }

    /// The name of the object.
    pub fn name(&self) -> String {
        // SAFETY: pj is valid. The name is a NUL-terminated string owned by the object (empty if NULL)
        unsafe {
            let p = sys::proj_get_name(self.pj.as_ptr());
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        }
    }
}
