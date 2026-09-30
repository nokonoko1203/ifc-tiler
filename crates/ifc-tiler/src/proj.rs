//! PROJ（proj-sys）の薄いRAIIの包み。`unsafe`と生ポインタはこのファイルに閉じ込める。
//!
//! コンテキスト（`Context`）とオブジェクト（`Object`）は`Drop`で破棄する。
//! オブジェクトはコンテキストの参照カウントを持ち、コンテキストより先に破棄される。

use std::ffi::{CStr, CString};
use std::ptr::{self, NonNull};
use std::rc::Rc;

use proj_sys as sys;

/// CRSの種類。判断に使うものだけを区別する。
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
        // SAFETY: 作った1つのコンテキストを1回だけ破棄する。オブジェクトはこれを保持しており、先に破棄済み
        unsafe { sys::proj_context_destroy(self.0.as_ptr()) };
    }
}

/// PROJのコンテキスト。複製は同じコンテキストを共有する。
#[derive(Clone)]
pub struct Context(Rc<RawContext>);

impl Context {
    /// 作れなければ`None`。
    pub fn new() -> Option<Self> {
        // SAFETY: 引数なし。返り値のNULLを確かめる
        let ctx = NonNull::new(unsafe { sys::proj_context_create() })?;
        Some(Self(Rc::new(RawContext(ctx))))
    }

    fn raw(&self) -> *mut sys::PJ_CONTEXT {
        self.0.0.as_ptr()
    }

    /// グリッドのネットワーク取得を有効・無効にする。
    pub fn set_network(&self, enable: bool) {
        // SAFETY: ctxは有効
        unsafe { sys::proj_context_set_enable_network(self.raw(), i32::from(enable)) };
    }

    /// グリッドのキャッシュを有効にする。
    pub fn enable_grid_cache(&self) {
        // SAFETY: ctxは有効
        unsafe { sys::proj_grid_cache_set_enable(self.raw(), 1) };
    }

    fn wrap(&self, pj: *mut sys::PJ) -> Option<Object> {
        NonNull::new(pj).map(|pj| Object { pj, ctx: self.clone() })
    }

    /// 定義文字列（`EPSG:4326`、WKTなど）からオブジェクトを作る。解釈できなければ`None`。
    pub fn create(&self, definition: &str) -> Option<Object> {
        let def = CString::new(definition).ok()?;
        // SAFETY: ctxは有効。defはNUL終端
        self.wrap(unsafe { sys::proj_create(self.raw(), def.as_ptr()) })
    }

    /// 水平のCRSと鉛直のCRSから複合CRSを作る。
    pub fn compound_crs(&self, name: &str, horizontal: &Object, vertical: &Object) -> Option<Object> {
        let name = CString::new(name).ok()?;
        // SAFETY: ctx・両オブジェクトは有効（オブジェクトはctxを保持する）
        self.wrap(unsafe {
            sys::proj_create_compound_crs(self.raw(), name.as_ptr(), horizontal.pj.as_ptr(), vertical.pj.as_ptr())
        })
    }

    /// `source`から`target`への変換を作る（PROJが候補から選ぶ）。
    pub fn crs_to_crs(&self, source: &Object, target: &Object) -> Option<Object> {
        // SAFETY: ctx・両オブジェクトは有効。領域・オプションは指定しない
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

/// PROJのオブジェクト（CRSや変換）。`Drop`で1回だけ破棄する。
pub struct Object {
    pj: NonNull<sys::PJ>,
    ctx: Context,
}

impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: 作った1つのオブジェクトを1回だけ破棄する。ctxはこのあとに破棄される
        unsafe { sys::proj_destroy(self.pj.as_ptr()) };
    }
}

impl Object {
    /// CRSの種類。
    pub fn kind(&self) -> Kind {
        // SAFETY: pjは有効
        match unsafe { sys::proj_get_type(self.pj.as_ptr()) } {
            sys::PJ_TYPE_PJ_TYPE_COMPOUND_CRS => Kind::Compound,
            sys::PJ_TYPE_PJ_TYPE_PROJECTED_CRS => Kind::Projected,
            sys::PJ_TYPE_PJ_TYPE_GEOGRAPHIC_2D_CRS => Kind::Geographic2d,
            sys::PJ_TYPE_PJ_TYPE_GEOGRAPHIC_3D_CRS => Kind::Geographic3d,
            _ => Kind::Other,
        }
    }

    /// 複合CRSの`index`番目の部分CRS。
    pub fn sub_crs(&self, index: i32) -> Option<Object> {
        // SAFETY: ctx・pjは有効
        self.ctx.wrap(unsafe { sys::proj_crs_get_sub_crs(self.ctx.raw(), self.pj.as_ptr(), index) })
    }

    /// 軸順を（東, 北）または（経度, 緯度）にそろえた変換。
    pub fn normalize_for_visualization(&self) -> Option<Object> {
        // SAFETY: ctx・pjは有効
        self.ctx.wrap(unsafe { sys::proj_normalize_for_visualization(self.ctx.raw(), self.pj.as_ptr()) })
    }

    /// 順方向に1点を変換する。結果が有限でなければ`None`。
    pub fn trans(&self, [x, y, z]: [f64; 3]) -> Option<[f64; 3]> {
        // SAFETY: pjは有効な変換
        let [x, y, z, _] =
            unsafe { sys::proj_trans(self.pj.as_ptr(), sys::PJ_DIRECTION_PJ_FWD, sys::proj_coord(x, y, z, 0.0)).v };
        [x, y, z].iter().all(|v| v.is_finite()).then_some([x, y, z])
    }

    /// 直前の変換で使われた操作。候補が1つだけの変換では`None`（その変換自身が操作）。
    pub fn last_used_operation(&self) -> Option<Object> {
        // SAFETY: pjは有効。返り値は複製なのでObjectが破棄する
        self.ctx.wrap(unsafe { sys::proj_trans_get_last_used_operation(self.pj.as_ptr()) })
    }

    /// グリッドを使わない近似（ballpark）を含む操作か。
    pub fn has_ballpark_transformation(&self) -> bool {
        // SAFETY: ctx・pjは有効
        unsafe { sys::proj_coordoperation_has_ballpark_transformation(self.ctx.raw(), self.pj.as_ptr()) == 1 }
    }

    /// オブジェクトの名前。
    pub fn name(&self) -> String {
        // SAFETY: pjは有効。名前はオブジェクトが持つNUL終端の文字列（NULLなら空）
        unsafe {
            let p = sys::proj_get_name(self.pj.as_ptr());
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        }
    }
}
