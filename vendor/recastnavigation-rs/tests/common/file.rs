use recastnavigation_rs::demo::{load_nav_mesh, save_nav_mesh};
use recastnavigation_rs::detour::DtNavMesh;
use std::env;
use std::env::consts::{ARCH, OS};
use std::error::Error;
use std::fs::{self, File};
use std::io::prelude::*;

pub fn compare_with_cpp_out(rs_mesh: &DtNavMesh, folder: &str, name: &str) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(format!("./expected/{}", folder)).unwrap();
    fs::create_dir_all(format!("./output/{}", folder)).unwrap();

    let rs_path = format!("./output/{0}/{1}_rs_out_{2}_{3}.bin", folder, name, OS, ARCH);
    save_nav_mesh(rs_mesh, &rs_path);

    let cpp_path = format!("./expected/{}/{}_cpp_out.bin", folder, name);
    let cpp_mesh = load_nav_mesh(&cpp_path).unwrap();

    if rs_mesh.params() != cpp_mesh.params() {
        return Err(format!("compare_with_cpp_out(params, {})", cpp_path).into());
    }

    if rs_mesh.max_tiles() != cpp_mesh.max_tiles() {
        return Err(format!("compare_with_cpp_out(max_tiles, {})", cpp_path).into());
    }

    for idx in 0..rs_mesh.max_tiles() {
        match (rs_mesh.get_tile(idx), cpp_mesh.get_tile(idx)) {
            (Some(rs_tile), Some(cpp_tile)) => {
                if rs_tile.header() != cpp_tile.header() {
                    return Err(format!("compare_with_cpp_out(tile[{}].header, {})", idx, cpp_path).into());
                }
                if rs_tile.data() != cpp_tile.data() {
                    return Err(format!("compare_with_cpp_out(tile[{}].data, {})", idx, cpp_path).into());
                }
            }
            (None, None) => {}
            _ => {
                return Err(format!("compare_with_cpp_out(tile[{}], {})", idx, cpp_path).into());
            }
        }
    }

    return Ok(());
}

#[cfg(feature = "rkyv")]
pub fn compare_with_rkyv<T>(folder: &str, name: &str, data: &T) -> Result<(), Box<dyn Error>>
where
    T: PartialEq + rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<30720>>,
    T::Archived: rkyv::Deserialize<T, rkyv::Infallible>,
{
    use miniz_oxide::deflate::compress_to_vec;
    use miniz_oxide::inflate::decompress_to_vec;
    use rkyv::ser::Serializer;
    use rkyv::{AlignedVec, Deserialize};

    fs::create_dir_all(format!("./expected/{}", folder)).unwrap();
    fs::create_dir_all(format!("./output/{}", folder)).unwrap();

    let to_expected = env::var("SAVE_TO_EXPECTED").is_ok();

    let mut serializer = rkyv::ser::serializers::AllocSerializer::<30720>::default();
    serializer.serialize_value(data)?;
    let current_buf = serializer.into_serializer().into_inner();
    let wbuf = compress_to_vec(&current_buf, 6);
    let path = if to_expected {
        format!("./expected/{0}/{1}.rkyv", folder, name)
    } else {
        format!("./output/{0}/{1}_{2}_{3}.rkyv", folder, name, OS, ARCH)
    };
    let mut file = File::create(path)?;
    file.write_all(&wbuf)?;

    if !to_expected {
        let path = format!("./expected/{0}/{1}.rkyv", folder, name);
        let mut file = File::open(&path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let mut rbuf = Vec::with_capacity(size as usize);
        file.read_to_end(&mut rbuf)?;
        let unaligned_buf = decompress_to_vec(&rbuf).map_err(|e| e.to_string())?;
        let mut expected_buf = AlignedVec::new();
        expected_buf.extend_from_slice(&unaligned_buf);

        let archived = unsafe { rkyv::archived_root::<T>(&expected_buf) };
        let mut deserializer = rkyv::Infallible::default();
        let expected = archived.deserialize(&mut deserializer)?;
        if data != &expected {
            return Err(format!("compare_with_rkyv({})", path).into());
        }
    }
    return Ok(());
}

#[cfg(not(feature = "rkyv"))]
pub fn compare_with_rkyv<T>(_folder: &str, _name: &str, _data: &T) -> Result<(), Box<dyn Error>> {
    return Ok(());
}
