//! Colour badges in Explorer (design pick 6A: the same set Finder shows).
//!
//! Explorer's own cfapi status column already shows "In cloud" (cloud), "On this
//! computer" (green tick) and syncing. Agent output, Shared and Conflict are custom
//! states set with `StorageProviderItemProperties`, using the definitions the sync
//! root registered. They need a StorageProviderSyncRootManager registration; in the
//! CfRegisterSyncRoot fallback this is a no-op (see `sync_root.rs`).

use std::path::Path;

use windows::core::{Interface, Result, HSTRING};
use windows::Foundation::Collections::IIterable;
use windows::Storage::Provider::{StorageProviderItemProperties, StorageProviderItemProperty};
use windows::Storage::{IStorageItem, StorageFile, StorageFolder};

use crate::conflict::Badge;

/// Sets (or clears, with an empty list) the badges on one item.
pub fn set(path: &Path, dir: bool, badges: &[Badge], icons_dir: &Path) -> Result<()> {
    crate::init_winrt();
    let p = HSTRING::from(path.as_os_str());
    let item: IStorageItem = if dir {
        StorageFolder::GetFolderFromPathAsync(&p)?.get()?.cast()?
    } else {
        StorageFile::GetFileFromPathAsync(&p)?.get()?.cast()?
    };
    let mut props = Vec::new();
    for b in badges {
        let prop = StorageProviderItemProperty::new()?;
        prop.SetId(b.id())?;
        prop.SetValue(&HSTRING::from(b.label()))?;
        prop.SetIconResource(&HSTRING::from(icons_dir.join(b.icon_file()).as_os_str()))?;
        props.push(Some(prop));
    }
    let list: IIterable<StorageProviderItemProperty> = IIterable::try_from(props)?;
    StorageProviderItemProperties::SetAsync(&item, &list)?.get()
}
