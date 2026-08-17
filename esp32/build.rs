fn main() {
    embuild::build::CargoArgs::from_comma_separated_list()
        .unwrap()
        .build()
        .unwrap();
}
