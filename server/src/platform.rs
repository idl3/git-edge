//! Every `js_sys::Reflect` use lives here (CONTRACTS.md A8). Nothing else in the crate may call Reflect.

use crate::error::Error;

pub fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

/// 16 random bytes as lowercase hex (repo_id, pack_id, push_id: section 8.2, 1.2).
pub fn hex16() -> Result<String, Error> {
    let crypto: web_sys::Crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into())
        .ok()
        .and_then(|v| wasm_bindgen::JsCast::dyn_into(v).ok())
        .ok_or_else(|| Error::Internal("no crypto".into()))?;
    let mut b = [0u8; 16];
    crypto
        .get_random_values_with_u8_array(&mut b)
        .map_err(|_| Error::Internal("getRandomValues".into()))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// The single permitted `ctx.id.name` read (section 8.3). Cross-check only; decides nothing.
/// worker 0.8.5 binds `ObjectId::name()` directly — no Reflect needed.
pub fn do_id_name(state: &worker::State) -> Option<String> {
    state.id().name()
}
