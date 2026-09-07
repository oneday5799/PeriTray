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
static TOAST_NOTIFIER: LazyLock<ToastNotifier> = LazyLock::new(|| {
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(crate::windows::AUMID))
        .expect("[toast] failed to create notifier")
});

#[cfg(target_os = "windows")]
static PREV_TOAST: OnceLock<Mutex<Option<ToastNotification>>> = OnceLock::new();

/// 显示系统 toast 通知。新通知出现时旧通知立即消失。
pub fn show_toast(title: &str, text: &str, icon: Option<&std::path::Path>) {
    #[cfg(target_os = "windows")]
    {
        // 隐藏旧通知
        if let Some(prev) = PREV_TOAST
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap()
            .take()
        {
            let _ = TOAST_NOTIFIER.Hide(&prev);
        }

        // 构建 XML → ToastNotification
        let notification = match build_notification(title, text, icon) {
            Ok(n) => n,
            Err(e) => {
                crate::process::append_log(&format!("[toast] build failed: {:?}", e));
                return;
            }
        };

        // 显示
        if let Err(e) = TOAST_NOTIFIER.Show(&notification) {
            crate::process::append_log(&format!("[toast] show failed: {:?}", e));
            return;
        }

        // 存储（保持 alive 以供下次 Hide）
        *PREV_TOAST.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(notification);
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
