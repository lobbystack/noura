//! Plugins' notifications in the system's notification center: the User
//! Notifications framework on macOS, toasts on Windows, and the desktop's
//! notification server over D-Bus on Linux. Choosing a notification or
//! one of its actions calls back into `app-core`, which hands the plugin a
//! kernel event.

use app_core::{NotificationChosen, Notifier, SystemNotification};

/// The notification center of this computer.
pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn show(
        &self,
        notification: SystemNotification,
        chosen: NotificationChosen,
    ) -> Result<(), String> {
        platform::show(notification, chosen)
    }

    fn withdraw(&self, key: &str) {
        platform::withdraw(key);
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, Once};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AllocAnyThread, define_class, msg_send};
    use objc2_foundation::{NSArray, NSBundle, NSError, NSSet, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
        UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
        UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    use super::{NotificationChosen, SystemNotification};

    /// What each shown notification calls back, and the action sets
    /// registered as categories, by category ID.
    #[derive(Default)]
    struct State {
        chosen: BTreeMap<String, NotificationChosen>,
        categories: BTreeMap<String, Vec<(String, String)>>,
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);
    static SETUP: Once = Once::new();

    fn with_state<R>(run: impl FnOnce(&mut State) -> R) -> R {
        let mut state = STATE.lock().unwrap_or_else(|error| error.into_inner());
        run(state.get_or_insert_with(State::default))
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the class
        // keeps no state of its own.
        #[unsafe(super(NSObject))]
        #[name = "NouraNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// You chose a notification, one of its actions, or closed it.
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion: &block2::DynBlock<dyn Fn()>,
            ) {
                let key = response.notification().request().identifier().to_string();
                let action = response.actionIdentifier();
                // SAFETY: both are read-only framework statics.
                let (dismissed, default) = unsafe {
                    (
                        action.isEqualToString(UNNotificationDismissActionIdentifier),
                        action.isEqualToString(UNNotificationDefaultActionIdentifier),
                    )
                };
                if !dismissed && let Some(chosen) = with_state(|state| state.chosen.remove(&key)) {
                    chosen((!default).then(|| action.to_string()));
                }
                completion.call(());
            }

            /// A notification arrives while Noura is in front: show it
            /// anyway, as a banner.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            // SAFETY: NSObject's init on a freshly allocated instance.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// The notification center, set up the first time. A process outside
    /// an app bundle, such as a development build, has none: asking for
    /// one there stops the process, so Noura shows nothing.
    fn center() -> Option<Retained<UNUserNotificationCenter>> {
        NSBundle::mainBundle().bundleIdentifier()?;
        let center = UNUserNotificationCenter::currentNotificationCenter();
        SETUP.call_once(|| {
            let delegate = Delegate::new();
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            // The center holds its delegate weakly; this one lives as long
            // as Noura.
            std::mem::forget(delegate);
            let answered = RcBlock::new(|granted: Bool, _error: *mut NSError| {
                if !granted.as_bool() {
                    tracing::info!("notifications are off for Noura in System Settings");
                }
            });
            center.requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &answered,
            );
        });
        Some(center)
    }

    /// The category whose buttons are `actions`, registered with the
    /// center when it's new.
    fn category(center: &UNUserNotificationCenter, actions: &[(String, String)]) -> String {
        let id = format!(
            "noura-actions:{}",
            actions
                .iter()
                .map(|(id, label)| format!("{id}={label}"))
                .collect::<Vec<_>>()
                .join("|")
        );
        let all = with_state(|state| {
            if state.categories.contains_key(&id) {
                return None;
            }
            state.categories.insert(id.clone(), actions.to_vec());
            Some(state.categories.clone())
        });
        if let Some(all) = all {
            let categories: Vec<Retained<UNNotificationCategory>> = all
                .iter()
                .map(|(id, actions)| {
                    let buttons: Vec<Retained<UNNotificationAction>> = actions
                        .iter()
                        .map(|(action, label)| {
                            UNNotificationAction::actionWithIdentifier_title_options(
                                &NSString::from_str(action),
                                &NSString::from_str(label),
                                UNNotificationActionOptions::Foreground,
                            )
                        })
                        .collect();
                    UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                        &NSString::from_str(id),
                        &NSArray::from_retained_slice(&buttons),
                        &NSArray::new(),
                        UNNotificationCategoryOptions::empty(),
                    )
                })
                .collect();
            center.setNotificationCategories(&NSSet::from_retained_slice(&categories));
        }
        id
    }

    pub(super) fn show(
        notification: SystemNotification,
        chosen: NotificationChosen,
    ) -> Result<(), String> {
        let Some(center) = center() else {
            tracing::info!("no notification center outside an app bundle");
            return Ok(());
        };
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        if !notification.actions.is_empty() {
            let category = category(&center, &notification.actions);
            content.setCategoryIdentifier(&NSString::from_str(&category));
        }
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification.key),
            &content,
            None,
        );
        with_state(|state| state.chosen.insert(notification.key.clone(), chosen));
        let added = RcBlock::new(|error: *mut NSError| {
            if !error.is_null() {
                tracing::warn!("the notification center refused a notification");
            }
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&added));
        Ok(())
    }

    pub(super) fn withdraw(key: &str) {
        with_state(|state| state.chosen.remove(key));
        let Some(center) = center() else {
            return;
        };
        let ids = NSArray::from_retained_slice(&[NSString::from_str(key)]);
        center.removeDeliveredNotificationsWithIdentifiers(&ids);
        center.removePendingNotificationRequestsWithIdentifiers(&ids);
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, Once};

    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{
        ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
    };
    use windows::core::{HSTRING, IInspectable, Interface};

    use super::{NotificationChosen, SystemNotification};

    /// The app ID toasts carry, which names Noura in the Action Center.
    const APP_ID: &str = "org.noura.desktop";
    /// The group every toast goes in, so a withdraw finds it by tag.
    const GROUP: &str = "noura";

    /// The toasts that show, by key, kept so their handlers stay alive.
    static SHOWN: Mutex<BTreeMap<String, ToastNotification>> = Mutex::new(BTreeMap::new());
    static SETUP: Once = Once::new();

    /// Name the app ID for this user, so toasts show "Noura" and Windows
    /// routes them to this process, which has no Start menu shortcut of
    /// its own while it runs from a build folder.
    fn register() {
        SETUP.call_once(|| {
            use windows::Win32::System::Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
                RegCreateKeyExW, RegSetValueExW,
            };
            use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
            let path = HSTRING::from(format!("Software\\Classes\\AppUserModelId\\{APP_ID}"));
            let mut key = HKEY::default();
            // SAFETY: a key under the current user's hive, closed below.
            let created = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    &path,
                    None,
                    None,
                    REG_OPTION_NON_VOLATILE,
                    KEY_WRITE,
                    None,
                    &mut key,
                    None,
                )
            };
            if created.is_ok() {
                let name: Vec<u16> = "Noura".encode_utf16().chain([0]).collect();
                // SAFETY: a NUL-terminated UTF-16 string as REG_SZ bytes.
                let bytes = unsafe {
                    std::slice::from_raw_parts(name.as_ptr().cast::<u8>(), name.len() * 2)
                };
                // SAFETY: `key` is open for writing.
                unsafe {
                    let _ = RegSetValueExW(
                        key,
                        &HSTRING::from("DisplayName"),
                        None,
                        REG_SZ,
                        Some(bytes),
                    );
                    let _ = RegCloseKey(key);
                }
            }
            // SAFETY: sets this process's app ID once, before any toast.
            if unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) }.is_err() {
                tracing::warn!("Windows didn't take Noura's app ID");
            }
        });
    }

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    /// A tag for `key` that fits the 64 characters Windows allows.
    fn tag(key: &str) -> String {
        let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
        format!("{hash:016x}")
    }

    fn toast(notification: &SystemNotification) -> windows::core::Result<ToastNotification> {
        let mut xml = format!(
            "<toast launch=\"\"><visual><binding template=\"ToastGeneric\"><text>{}</text>",
            escape(&notification.title)
        );
        if !notification.body.is_empty() {
            xml.push_str(&format!("<text>{}</text>", escape(&notification.body)));
        }
        xml.push_str("</binding></visual>");
        if !notification.actions.is_empty() {
            xml.push_str("<actions>");
            for (id, label) in &notification.actions {
                xml.push_str(&format!(
                    "<action content=\"{}\" arguments=\"{}\" activationType=\"foreground\"/>",
                    escape(label),
                    escape(id)
                ));
            }
            xml.push_str("</actions>");
        }
        xml.push_str("</toast>");
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        toast.SetTag(&HSTRING::from(tag(&notification.key)))?;
        toast.SetGroup(&HSTRING::from(GROUP))?;
        Ok(toast)
    }

    pub(super) fn show(
        notification: SystemNotification,
        chosen: NotificationChosen,
    ) -> Result<(), String> {
        register();
        let shown = || -> windows::core::Result<ToastNotification> {
            let toast = toast(&notification)?;
            let key = notification.key.clone();
            toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_, args| {
                    let action = args
                        .as_ref()
                        .and_then(|args| args.cast::<ToastActivatedEventArgs>().ok())
                        .and_then(|args| args.Arguments().ok())
                        .map(|arguments| arguments.to_string())
                        .filter(|arguments| !arguments.is_empty());
                    SHOWN
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(&key);
                    chosen(action);
                    Ok(())
                },
            ))?;
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?
                .Show(&toast)?;
            Ok(toast)
        };
        let toast = shown().map_err(|error| error.message())?;
        SHOWN
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(notification.key, toast);
        Ok(())
    }

    pub(super) fn withdraw(key: &str) {
        SHOWN
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(key);
        if let Ok(history) = ToastNotificationManager::History() {
            let _ = history.RemoveGroupedTagWithId(
                &HSTRING::from(tag(key)),
                &HSTRING::from(GROUP),
                &HSTRING::from(APP_ID),
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{NotificationChosen, SystemNotification};

    pub(super) fn show(
        notification: SystemNotification,
        chosen: NotificationChosen,
    ) -> Result<(), String> {
        crate::linux_portal::Portal::get().notify(notification, chosen);
        Ok(())
    }

    pub(super) fn withdraw(key: &str) {
        crate::linux_portal::Portal::get().withdraw(key);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::{NotificationChosen, SystemNotification};

    pub(super) fn show(_: SystemNotification, _: NotificationChosen) -> Result<(), String> {
        Ok(())
    }

    pub(super) fn withdraw(_: &str) {}
}
