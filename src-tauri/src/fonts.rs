use std::collections::HashMap;

use font_kit::family_name::FamilyName;
use font_kit::properties::Properties;
use font_kit::source::SystemSource;

/// 逐个字体名查询系统是否安装。
/// 走系统字体框架（macOS CoreText / Windows DirectWrite / Linux fontconfig）
/// 的权威索引；WebView 内的 canvas 测量与 document.fonts.check 对本地字体
/// 判定不可靠（实测 WKWebView 将系统自带的 Menlo 误报为未安装），故统一由后端判定。
#[tauri::command]
pub async fn check_fonts(names: Vec<String>) -> Result<HashMap<String, bool>, String> {
    crate::slow_span!("check_fonts");
    // 查询是对内存字体索引的匹配，很轻，但仍放阻塞线程池避免占用 async worker
    tauri::async_runtime::spawn_blocking(move || {
        let source = SystemSource::new();
        let props = Properties::new();
        names
            .into_iter()
            .map(|name| {
                let installed = source
                    .select_best_match(&[FamilyName::Title(name.clone())], &props)
                    .is_ok();
                (name, installed)
            })
            .collect::<HashMap<_, _>>()
    })
    .await
    .map_err(|e| format!("check_fonts join failed: {e}"))
}
