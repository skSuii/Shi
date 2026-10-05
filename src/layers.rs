//! 图层辅助：读取/写回内核 Layer（通过访问器，不再走 serde 往返）。
//! 图层数据始终保存在内核 Document 中（对齐内核数据模型）。

use cadrs::data_structure::layer::{Color, LayerVisibility};
use cadrs::data_structure::Layer;

#[derive(Clone)]
pub struct LayerInfo {
    pub name: String,
    pub color: (u8, u8, u8),
    pub visible: bool,
    pub locked: bool,
}

/// 读取图层信息
pub fn read(layer: &Layer) -> LayerInfo {
    let c = layer.color();
    LayerInfo {
        name: layer.name().to_string(),
        color: (c.red, c.green, c.blue),
        visible: layer.is_visible(),
        locked: layer.is_locked(),
    }
}

/// 把 LayerInfo 写回图层
pub fn write(layer: &mut Layer, info: &LayerInfo) {
    layer.set_name(info.name.clone());
    layer.set_color(Color {
        red: info.color.0,
        green: info.color.1,
        blue: info.color.2,
    });
    let vis = if !info.visible && info.locked {
        LayerVisibility::Frozen
    } else if !info.visible {
        LayerVisibility::Hidden
    } else if info.locked {
        LayerVisibility::Locked
    } else {
        LayerVisibility::Visible
    };
    layer.set_visibility(vis);
}
