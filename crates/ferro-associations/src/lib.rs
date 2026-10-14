pub mod config;
pub mod view;

pub use config::{
    default_associations, load_associations, parse_desktop_entry, parse_desktop_entry_content, resolve_command_for_path, save_associations,
    shell_quote, strip_field_codes, AssociationEntry,
};
