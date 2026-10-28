pub mod config;
pub mod thumbs;
pub mod trash;
pub mod view;

pub use config::{
    default_associations, file_override, has_file_code, load_associations, open_with_command, parse_desktop_entry, parse_desktop_entry_content,
    resolve_command_for_path, save_associations, set_file_override, shell_quote, strip_field_codes, trusted_launcher, AssociationEntry,
};
