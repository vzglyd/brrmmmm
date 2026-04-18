use brrmmmm::persistence::wasm_identity;

#[test]
fn wasm_identity_empty_slice_is_fnv1a_offset_basis() {
    // FNV-1a 64-bit: no bytes XOR'd, so result is the offset basis in hex.
    assert_eq!(wasm_identity(&[]), "cbf29ce484222325");
}

#[test]
fn wasm_identity_is_deterministic() {
    let data = b"brrmmmm sidecar runtime";
    assert_eq!(wasm_identity(data), wasm_identity(data));
}

#[test]
fn wasm_identity_output_is_16_lowercase_hex_chars() {
    let result = wasm_identity(b"hello");
    assert_eq!(result.len(), 16);
    assert!(
        result.chars().all(|c| c.is_ascii_hexdigit()),
        "non-hex chars in: {result}"
    );
    assert!(
        result.chars().all(|c| !c.is_ascii_uppercase()),
        "uppercase chars in: {result}"
    );
}

#[test]
fn wasm_identity_different_inputs_produce_different_hashes() {
    assert_ne!(wasm_identity(b"alpha"), wasm_identity(b"beta"));
}

#[test]
fn wasm_identity_single_byte_change_produces_different_hash() {
    let a = b"hello";
    let b = b"hellp";
    assert_ne!(wasm_identity(a), wasm_identity(b));
}
