use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::rc::Rc;

use anyhow::{anyhow, Context, Result};

const OFFSET: i32 = 150;
const GRID_SIZE: i32 = 300;

const SAVE_LOCATION: &str =
    "/home/vivax/.local/share/Steam/steamapps/common/Logic World/saves/AAAAAAAAAA/data.logicworld";

struct Version(i32, i32, i32, i32);
impl std::fmt::Debug for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Version({}.{}.{}.{})", self.0, self.1, self.2, self.3)
    }
}

struct Vec3 {
    x: i32,
    y: i32,
    z: i32,
}
impl std::fmt::Debug for Vec3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {}, {})", self.x, self.y, self.z)
    }
}

struct Quat {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}
impl std::fmt::Debug for Quat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {}, {}, {})", self.x, self.y, self.z, self.w)
    }
}

#[derive(Debug)]
enum CustomData {
    Unknown(Vec<u8>),
    Switch {
        color: (u8, u8, u8),
        on: bool,
    },
    Display {
        // never seems to go above 16, but I assume they are using a C# int?
        color_mode: u32,
    },
}

#[derive(Debug)]
struct Component {
    address: u32,
    parent: u32,
    id: Rc<str>,
    position: Vec3,
    rotation: Quat,
    inputs: Vec<i32>,
    outputs: Vec<i32>,
    custom_data: CustomData,
}

#[derive(Debug)]
enum PegType {
    Input,
    Output,
}

#[derive(Debug)]
struct PegAddress {
    type_: PegType,
    component: u32,
    index: i32,
}

#[derive(Debug)]
struct Wire {
    start: PegAddress,
    end: PegAddress,
    state_id: i32,
    rotation: f32,
}

struct States(Vec<u8>);
impl std::fmt::Debug for States {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[...]")
    }
}

#[derive(Debug)]
struct SaveFile {
    game_version: Version,
    mod_versions: HashMap<Box<str>, Version>,
    comp_map: CompMap,
    components: Vec<Component>,
    wires: Vec<Wire>,
    states: States,
    highest_state_id: i32,
    highest_address: u32,
}

impl SaveFile {
    fn clear_out(&mut self) {
        self.comp_map = CompMap::with_capacity(0);
        self.components.clear();
        self.wires.clear();
        self.highest_state_id = 0;
        self.highest_address = 1;
    }

    fn get_free_state_id(&mut self) -> i32 {
        self.highest_state_id += 1;

        if self.highest_state_id / 8 >= self.states.0.len() as i32 {
            self.states.0.push(0);
        }

        self.highest_state_id
    }
    fn get_free_address(&mut self) -> u32 {
        self.highest_address += 1;
        self.highest_address
    }
}

#[derive(Debug)]
struct CompMap {
    k_ids: HashMap<u16, Rc<str>>,
    k_name: HashMap<Rc<str>, u16>,
}

impl CompMap {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            k_ids: HashMap::with_capacity(capacity),
            k_name: HashMap::with_capacity(capacity),
        }
    }

    fn insert(&mut self, id: u16, name: Rc<str>) {
        self.k_ids.insert(id, name.clone());
        self.k_name.insert(name, id);
    }

    fn get_id(&self, id: u16) -> Result<Rc<str>> {
        self.k_ids
            .get(&id)
            .map(Rc::clone)
            .ok_or(anyhow!("Missing id in mapping"))
    }

    fn get_name(&self, name: Rc<str>) -> Result<u16> {
        self.k_name
            .get(&name)
            .copied()
            .ok_or(anyhow!("Missing id in mapping"))
    }

    fn ensure(&mut self, name: &str) {
        if !self.k_name.contains_key(name) {
            let new_id = self.k_ids.keys().max().unwrap_or(&0) + 1;
            self.insert(new_id, name.into());
        }
    }
}

struct Parser {
    file: fs::File,
    id_mapping: CompMap,
    highest_state_id: i32,
}

impl Parser {
    fn new(save_file: fs::File) -> Self {
        Self {
            file: save_file,
            id_mapping: CompMap::with_capacity(0),
            highest_state_id: 0,
        }
    }

    fn parse_save(mut self) -> Result<SaveFile> {
        self.validate_header().context("Validating header")?;
        self.validate_version().context("Validating version")?;
        let game_version = self.read_version().context("Reading game version")?;
        self.validate_save_type().context("Validating save type")?;

        let num_components = self.read_int().context("Reading num components")?;
        let num_wires = self.read_int().context("Reading num wires")?;

        let mod_versions = self.read_mod_versions().context("Reading mods")?;
        self.read_comp_map().context("reading component map")?;

        let mut components = Vec::with_capacity(num_components as usize);
        for _ in 0..num_components {
            components.push(self.read_component().context("reading component")?);
        }

        let mut wires = Vec::with_capacity(num_wires as usize);
        for _ in 0..num_wires {
            wires.push(self.read_wire().context("reading wire")?);
        }

        let num_states = self.read_int().context("reading num states")?;
        let mut states = Vec::with_capacity(num_states as usize);
        for _ in 0..num_states {
            states.push(self.read_byte().context("reading states byte")?);
        }

        self.validate_footer().context("validating footer")?;

        let highest_address = components
            .iter()
            .map(|comp| comp.address)
            .max()
            .unwrap_or(1);

        Ok(SaveFile {
            game_version,
            mod_versions,
            comp_map: self.id_mapping,
            components,
            wires,
            states: States(states),
            highest_state_id: self.highest_state_id,
            highest_address,
        })
    }

    fn read_wire(&mut self) -> Result<Wire> {
        let start = self.read_peg_address()?;
        let end = self.read_peg_address()?;
        let state_id = self.read_state_id()?;
        let rotation = self.read_float()?;

        Ok(Wire {
            start,
            end,
            state_id,
            rotation,
        })
    }

    fn read_peg_address(&mut self) -> Result<PegAddress> {
        let type_ = self.read_byte()?;
        let type_ = match type_ {
            1 => PegType::Input,
            2 => PegType::Output,
            _ => return Err(anyhow!("Invalid peg type, ${type_}")),
        };

        let component = self.read_address()?;
        let index = self.read_int()?;

        Ok(PegAddress {
            type_,
            component,
            index,
        })
    }

    fn read_component(&mut self) -> Result<Component> {
        let address = self.read_address()?;
        let parent = self.read_address()?;

        let id = self.read_id()?;
        let id = self.id_mapping.get_id(id)?;

        let position = self.read_pos()?;
        let rotation = self.read_rot()?;

        let input_count = self.read_int()?;
        let mut inputs = Vec::with_capacity(input_count as usize);
        for _ in 0..input_count {
            inputs.push(self.read_state_id()?);
        }
        let output_count = self.read_int()?;
        let mut outputs = Vec::with_capacity(input_count as usize);
        for _ in 0..output_count {
            outputs.push(self.read_state_id()?);
        }

        let custom_data_amount = self.read_int()?.max(0);
        let mut data = vec![0u8; custom_data_amount as usize];
        self.file.read_exact(&mut data)?;
        let custom_data = self.parse_custom_data(&id, data)?;

        Ok(Component {
            address,
            parent,
            id,
            position,
            rotation,
            inputs,
            outputs,
            custom_data,
        })
    }

    fn parse_custom_data(&self, id: &str, data: Vec<u8>) -> Result<CustomData> {
        Ok(match id {
            "MHG.Switch" | "MHG.Button" => CustomData::Switch {
                color: (data[0], data[1], data[2]),
                on: data[3] != 0,
            },
            "MHG.StandingDisplay" => CustomData::Display {
                color_mode: u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            },
            _ => CustomData::Unknown(data),
        })
    }

    fn read_pos(&mut self) -> Result<Vec3> {
        Ok(Vec3 {
            x: self.read_int()?,
            y: self.read_int()?,
            z: self.read_int()?,
        })
    }
    fn read_rot(&mut self) -> Result<Quat> {
        Ok(Quat {
            x: self.read_float()?,
            y: self.read_float()?,
            z: self.read_float()?,
            w: self.read_float()?,
        })
    }

    fn read_comp_map(&mut self) -> Result<()> {
        let count = self.read_int().context("reading comp map count")?;
        self.id_mapping = CompMap::with_capacity(count as usize);

        for _ in 0..count {
            let id = self.read_id().context("reading number")?;
            let name = self.read_string().context("reading text")?;
            self.id_mapping.insert(id, name.into());
        }

        Ok(())
    }

    fn read_mod_versions(&mut self) -> Result<HashMap<Box<str>, Version>> {
        let count = self.read_int()?;
        let mut mapping = HashMap::with_capacity(count as usize);
        for _ in 0..count {
            let name = self.read_string()?;
            let version = self.read_version()?;
            mapping.insert(name, version);
        }

        Ok(mapping)
    }

    fn validate_header(&mut self) -> Result<()> {
        let mut header = [0u8; 16];
        self.file.read_exact(&mut header)?;
        let header = String::from_utf8(header.into())?;
        if header != "Logic World save" {
            Err(anyhow!("Invalid header, '{header}'"))
        } else {
            Ok(())
        }
    }
    fn validate_footer(&mut self) -> Result<()> {
        let mut header = [0u8; 16];
        self.file.read_exact(&mut header)?;
        let header = String::from_utf8(header.into())?;
        if header != "redstone sux lol" {
            Err(anyhow!("Invalid header, '{header}'"))
        } else {
            Ok(())
        }
    }

    fn validate_version(&mut self) -> Result<()> {
        let version = self.read_byte()?;
        if version == 7 {
            Ok(())
        } else {
            Err(anyhow!("Invalid save format version {version}"))
        }
    }

    fn read_version(&mut self) -> Result<Version> {
        Ok(Version(
            self.read_int()?,
            self.read_int()?,
            self.read_int()?,
            self.read_int()?,
        ))
    }

    fn validate_save_type(&mut self) -> Result<()> {
        let save_type = self.read_byte()?;
        if save_type == 1 {
            Ok(())
        } else {
            Err(anyhow!("Invalid save type ${save_type}"))
        }
    }

    fn read_string(&mut self) -> Result<Box<str>> {
        let count = self.read_int()?;
        let mut data = vec![0u8; count as usize];
        self.file.read_exact(&mut data)?;
        let data = String::from_utf8(data)?.into_boxed_str();
        Ok(data)
    }

    fn read_byte(&mut self) -> Result<u8> {
        Ok(self.read_n_bytes::<1>()?[0])
    }

    fn read_float(&mut self) -> Result<f32> {
        let data = self.read_n_bytes::<4>()?;
        Ok(f32::from_le_bytes(data))
    }
    fn read_int(&mut self) -> Result<i32> {
        let data = self.read_n_bytes::<4>()?;
        Ok(i32::from_le_bytes(data))
    }
    fn read_state_id(&mut self) -> Result<i32> {
        let id = self.read_int()?;
        self.highest_state_id = self.highest_state_id.max(id);
        Ok(id)
    }
    fn read_address(&mut self) -> Result<u32> {
        let data = self.read_n_bytes::<4>()?;
        Ok(u32::from_le_bytes(data))
    }
    fn read_id(&mut self) -> Result<u16> {
        let data = self.read_n_bytes::<2>()?;
        Ok(u16::from_le_bytes(data))
    }

    fn read_n_bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut data = [0u8; N];
        self.file.read_exact(&mut data)?;
        Ok(data)
    }
}

struct Writer {
    result: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Writer { result: Vec::new() }
    }

    fn write(mut self, save: SaveFile) -> Result<Vec<u8>> {
        self.write_raw_string("Logic World save");

        self.result.push(7);
        self.write_version(&save.game_version);
        self.result.push(1);
        self.write_int(save.components.len() as i32);
        self.write_int(save.wires.len() as i32);

        self.write_int(save.mod_versions.len() as i32);
        for (name, version) in save.mod_versions.iter() {
            self.write_string(name);
            self.write_version(version);
        }

        self.write_int(save.comp_map.k_ids.len() as i32);
        for (text_id, num_id) in save.comp_map.k_name.iter() {
            self.write_id(*num_id);
            self.write_string(text_id);
        }

        for comp in &save.components {
            self.write_component(comp, &save.comp_map)?;
        }
        for wire in &save.wires {
            self.write_wire(wire);
        }

        self.write_int(save.states.0.len() as i32);
        self.result.extend(save.states.0);

        self.write_raw_string("redstone sux lol");

        Ok(self.result)
    }

    fn write_wire(&mut self, wire: &Wire) {
        self.write_peg_address(&wire.start);
        self.write_peg_address(&wire.start);
        self.write_int(wire.state_id);
        self.write_float(wire.rotation);
    }

    fn write_peg_address(&mut self, address: &PegAddress) {
        match address.type_ {
            PegType::Input => self.result.push(1),
            PegType::Output => self.result.push(2),
        }
        self.write_address(address.component);
        self.write_int(address.index);
    }

    fn write_component(&mut self, comp: &Component, mapping: &CompMap) -> Result<()> {
        self.write_address(comp.address);
        self.write_address(comp.parent);
        self.write_id(mapping.get_name(comp.id.clone())?);

        self.write_int(comp.position.x);
        self.write_int(comp.position.y);
        self.write_int(comp.position.z);

        self.write_float(comp.rotation.x);
        self.write_float(comp.rotation.y);
        self.write_float(comp.rotation.z);
        self.write_float(comp.rotation.w);

        self.write_int(comp.inputs.len() as i32);
        for inp in &comp.inputs {
            self.write_int(*inp);
        }
        self.write_int(comp.outputs.len() as i32);
        for inp in &comp.outputs {
            self.write_int(*inp);
        }

        let custom_data = self.do_customdata(&comp.custom_data);
        self.write_int(custom_data.len() as i32);
        self.result.extend(custom_data);

        Ok(())
    }

    fn do_customdata(&mut self, data: &CustomData) -> Vec<u8> {
        match data {
            CustomData::Unknown(data) => data.clone(),
            CustomData::Display { color_mode } => color_mode.to_le_bytes().to_vec(),
            CustomData::Switch { color, on } => {
                vec![color.0, color.1, color.2, if *on { 1 } else { 0 }]
            }
        }
    }

    fn write_version(&mut self, version: &Version) {
        self.write_int(version.0);
        self.write_int(version.1);
        self.write_int(version.2);
        self.write_int(version.3);
    }

    fn write_string(&mut self, data: &str) {
        let bytes = data.as_bytes();
        self.write_int(bytes.len() as i32);
        self.result.extend(bytes);
    }

    fn write_id(&mut self, data: u16) {
        self.result.extend(data.to_le_bytes());
    }

    fn write_float(&mut self, data: f32) {
        self.result.extend(data.to_le_bytes());
    }

    fn write_address(&mut self, data: u32) {
        self.result.extend(data.to_le_bytes());
    }

    fn write_int(&mut self, data: i32) {
        self.result.extend(data.to_le_bytes());
    }

    fn write_raw_string(&mut self, data: &str) {
        self.result.extend(data.as_bytes());
    }
}

fn main() -> Result<()> {
    println!("Reading save");
    let save_file = fs::File::open(SAVE_LOCATION)?;
    let parser = Parser::new(save_file);
    let mut result = parser.parse_save()?;
    result.clear_out();

    println!("Modifying save");
    result.comp_map.ensure("MHG.Button");
    for x in 0..10 {
        for y in 0..10 {
            let comp = Component {
                address: result.get_free_address(),
                parent: 0,
                id: "MHG.Button".into(),
                position: Vec3 {
                    x: OFFSET + x * GRID_SIZE,
                    y: (x + y) * 100,
                    z: OFFSET + y * GRID_SIZE,
                },
                rotation: Quat {
                    x: 0.,
                    y: 0.,
                    z: 0.,
                    w: 0.,
                },
                inputs: vec![],
                outputs: vec![result.get_free_state_id()],
                custom_data: CustomData::Switch {
                    color: (x as u8 * 10, y as u8 * 10, 0),
                    on: false,
                },
            };
            result.components.push(comp);
        }
    }

    println!("Generating binary");
    let writer = Writer::new();
    let result = writer.write(result)?;

    println!("Writing save");
    fs::write(SAVE_LOCATION, result)?;

    Ok(())
}
