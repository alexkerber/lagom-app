use core_foundation::{
    base::{CFType, TCFType},
    data::CFData,
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use security_framework_sys::{
    access_control::kSecAttrAccessibleWhenUnlockedThisDeviceOnly, item::kSecValueData,
};
use zeroize::Zeroizing;

// security-framework-sys exports the accessibility values but omits their key.
extern "C" {
    static kSecAttrAccessible: CFStringRef;
}

#[cfg(target_os = "ios")]
pub(super) fn read(service: &str, account: &str) -> Option<Zeroizing<String>> {
    use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
    use security_framework_sys::{
        item::{kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword},
        keychain_item::SecItemUpdate,
    };

    let mut items = ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(service)
        .account(account)
        .load_attributes(true)
        .load_data(true)
        .search()
        .ok()?;
    let SearchResult::Dict(item) = items.pop()? else {
        return None;
    };
    read_item(&item, |old, changes| unsafe {
        let string = CFString::wrap_under_get_rule;
        let query = CFDictionary::from_CFType_pairs(&[
            (string(kSecClass), string(kSecClassGenericPassword)),
            (string(kSecAttrService), CFString::from(service)),
            (string(kSecAttrAccount), CFString::from(account)),
            (string(kSecAttrAccessible), old),
        ]);
        SecItemUpdate(query.as_concrete_TypeRef(), changes.as_concrete_TypeRef())
    })
}

fn read_item(
    item: &CFDictionary,
    update: impl FnOnce(CFString, CFDictionary<CFString, CFString>) -> i32,
) -> Option<Zeroizing<String>> {
    let value = |key: CFStringRef| {
        item.find(key.cast())
            .map(|v| unsafe { CFType::wrap_under_get_rule(*v) })
    };
    let data = value(unsafe { kSecValueData })?.downcast_into::<CFData>()?;
    let password = Zeroizing::new(std::str::from_utf8(data.bytes()).ok()?.to_owned());
    let accessible = unsafe { CFString::wrap_under_get_rule(kSecAttrAccessible) };
    let current =
        unsafe { CFString::wrap_under_get_rule(kSecAttrAccessibleWhenUnlockedThisDeviceOnly) };
    if let Some(old) =
        value(accessible.as_concrete_TypeRef()).and_then(|v| v.downcast_into::<CFString>())
    {
        if old != current {
            // A failed update leaves the item available for the next read.
            let _ = update(
                old,
                CFDictionary::from_CFType_pairs(&[(accessible, current)]),
            );
        }
    }
    Some(password)
}

#[cfg(test)]
mod tests {
    use super::*;
    use security_framework_sys::access_control::kSecAttrAccessibleWhenUnlocked;

    #[test]
    fn keychain_read_keeps_secret_and_updates_only_legacy_accessibility() {
        let key = unsafe { CFString::wrap_under_get_rule(kSecAttrAccessible) };
        let current =
            unsafe { CFString::wrap_under_get_rule(kSecAttrAccessibleWhenUnlockedThisDeviceOnly) };
        let legacy = unsafe { CFString::wrap_under_get_rule(kSecAttrAccessibleWhenUnlocked) };
        for class in [&legacy, &current] {
            let item = CFDictionary::from_CFType_pairs(&[
                (key.clone(), class.as_CFType()),
                (
                    unsafe { CFString::wrap_under_get_rule(kSecValueData) },
                    CFData::from_buffer(b"fixture").into_CFType(),
                ),
            ]);
            let mut attempted = false;
            let password = read_item(&item.to_untyped(), |old, changes| {
                attempted = true;
                assert_eq!(old, legacy);
                assert_eq!(changes.len(), 1);
                assert_eq!(*changes.get(&key), current);
                -1
            });
            assert_eq!(attempted, class == &legacy);
            assert_eq!(&**password.unwrap(), "fixture");
        }
    }
}
