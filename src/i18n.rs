
use std::sync::Mutex;
use lazy_static::lazy_static;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Zh,
}

lazy_static! {
    static ref CURRENT_LANG: Mutex<Language> = Mutex::new(Language::Zh); // Default to Chinese
}

pub fn set_language(lang: Language) {
    *CURRENT_LANG.lock().unwrap() = lang;
}

pub fn get_language() -> Language {
    *CURRENT_LANG.lock().unwrap()
}

pub fn t(key: &str) -> String {
    let lang = *CURRENT_LANG.lock().unwrap();
    let val = match lang {
        Language::En => get_en(key),
        Language::Zh => get_zh(key),
    };
    if val.is_empty() {
        key.to_string()
    } else {
        val.to_string()
    }
}

fn get_zh(key: &str) -> &'static str {
    match key {
        "app_title" => "自动 A-B 循环播放器",
        "open_file" => "打开文件...",
        "drag_drop" => "拖拽音频文件到此处",
        "loading" => "正在加载...",
        "detecting" => "正在检测循环点...",
        "reading" => "读取文件中...",
        "unknown_title" => "未知标题",
        "unknown_artist" => "未知艺术家",
        "unknown_album" => "未知专辑",
        "loop_found" => "发现循环点",
        "confidence" => "置信度",
        "fade_out_loop" => "检测到淡出循环！",
        "no_loop" => "未检测到循环，正常播放。",
        "low_accuracy" => "匹配精度较低，结果可能不准确。",
        "play" => "播放",
        "stop" => "停止",
        "volume" => "音量",
        "loop_count" => "循环次数",
        "infinite" => "无限",
        "export" => "导出...",
        "exporting" => "正在导出...",
        "export_success" => "导出成功！",
        "export_fail" => "导出失败：",
        "save_file" => "保存文件",
        _ => "", // Return empty or fallback
    }
}

fn get_en(key: &str) -> &'static str {
    match key {
        "app_title" => "Auto A-B Loop Player",
        "open_file" => "Open File...",
        "drag_drop" => "Drag & Drop Audio File Here",
        "loading" => "Loading...",
        "detecting" => "Detecting Loop Points...",
        "reading" => "Reading file...",
        "unknown_title" => "Unknown Title",
        "unknown_artist" => "Unknown Artist",
        "unknown_album" => "Unknown Album",
        "loop_found" => "Loop Found",
        "confidence" => "Confidence",
        "fade_out_loop" => "Fade-Out Loop Detected!",
        "no_loop" => "No loop detected. Normal playback.",
        "low_accuracy" => "Low accuracy match - result might be incorrect.",
        "play" => "Play",
        "stop" => "Stop",
        "volume" => "Volume",
        "loop_count" => "Loop Count",
        "infinite" => "Infinite",
        "export" => "Export...",
        "exporting" => "Exporting...",
        "export_success" => "Export Successful!",
        "export_fail" => "Export Failed: ",
        "save_file" => "Save File",
        _ => "",
    }
}
