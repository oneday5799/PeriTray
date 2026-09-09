// ── 系统 toast 通知统一入口 ──
// 新通知出现时旧通知立即消失（通过 ToastNotifier::Hide）。

#[cfg(target_os = "windows")]
use std::path::Path;
#[cfg(target_os = "windows")]
use std::sync::{LazyLock, Mutex, OnceLock};
#[cfg(target_os = "windows")]
use windows::core::{h, HSTRING};
#[cfg(target_os = "windows")]
use windows::Data::Xml::Dom::XmlDocument;
#[cfg(target_os = "windows")]
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};

#[cfg(target_os = "windows")]
use crate::{standard_log, verbose_log};
static TOAST_NOTIFIER: LazyLock<Option<ToastNotifier>> = LazyLock::new(|| {
    match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(crate::windows::AUMID))
    {
        Ok(notifier) => Some(notifier),
        Err(e) => {
            verbose_log!("[toast] failed to create notifier: {:?}", e);
            None
        }
    }
});

#[cfg(target_os = "windows")]
static PREV_TOAST: OnceLock<Mutex<Option<ToastNotification>>> = OnceLock::new();

/// 显示系统 toast 通知。新通知出现时旧通知立即消失。
pub fn show_toast(title: &str, text: &str, icon: Option<&std::path::Path>) {
    #[cfg(target_os = "windows")]
    {
        let Some(ref notifier) = *TOAST_NOTIFIER else {
            return;
        };

        // 隐藏旧通知（使用 lock_unpoisoned 防止 mutex 中毒 panic）
        if let Some(prev) =
            crate::state::lock_unpoisoned(PREV_TOAST.get_or_init(|| Mutex::new(None))).take()
        {
            let _ = notifier.Hide(&prev);
        }

        // 构建 XML → ToastNotification
        let notification = match build_notification(title, text, icon) {
            Ok(n) => n,
            Err(e) => {
                standard_log!("[toast] build failed: {:?}", e);
                return;
            }
        };

        // 显示
        if let Err(e) = notifier.Show(&notification) {
            standard_log!("[toast] show failed: {:?}", e);
            return;
        }

        // 存储（保持 alive 以供下次 Hide）
        *crate::state::lock_unpoisoned(PREV_TOAST.get_or_init(|| Mutex::new(None))) =
            Some(notification);
    }
}

#[cfg(target_os = "windows")]
fn build_notification(
    title: &str,
    text: &str,
    icon: Option<&Path>,
) -> windows::core::Result<ToastNotification> {
    let xml_doc = XmlDocument::new()?;
    let xml_toast = xml_doc.CreateElement(h!("toast"))?;
    let xml_visual = xml_doc.CreateElement(h!("visual"))?;
    let xml_binding = xml_doc.CreateElement(h!("binding"))?;
    xml_binding.SetAttribute(h!("template"), h!("ToastGeneric"))?;

    // title
    let el_title = xml_doc.CreateElement(h!("text"))?;
    el_title.SetAttribute(h!("id"), h!("1"))?;
    el_title.SetInnerText(&title.into())?;
    xml_binding.AppendChild(&el_title)?;

    // text
    let el_text = xml_doc.CreateElement(h!("text"))?;
    el_text.SetAttribute(h!("id"), h!("2"))?;
    el_text.SetInnerText(&text.into())?;
    xml_binding.AppendChild(&el_text)?;

    // icon
    if let Some(path) = icon {
        let el_icon = xml_doc.CreateElement(h!("image"))?;
        el_icon.SetAttribute(h!("id"), h!("1"))?;
        el_icon.SetAttribute(h!("src"), &format!("file:///{}", path.display()).into())?;
        el_icon.SetAttribute(h!("alt"), h!(""))?;
        el_icon.SetAttribute(h!("placement"), h!("appLogoOverride"))?;
        el_icon.SetAttribute(h!("hint-crop"), h!("circle"))?;
        xml_binding.AppendChild(&el_icon)?;
    }

    xml_visual.AppendChild(&xml_binding)?;
    xml_toast.AppendChild(&xml_visual)?;

    // silent audio
    let xml_audio = xml_doc.CreateElement(h!("audio"))?;
    xml_audio.SetAttribute(h!("silent"), h!("true"))?;
    xml_toast.AppendChild(&xml_audio)?;

    xml_doc.AppendChild(&xml_toast)?;

    ToastNotification::CreateToastNotification(&xml_doc)
}
