/** 应用元信息统一出口：侧栏、设置「关于」等处共用，避免版本号分散硬编码 */

/** 构建期由 vite.config.ts 从 src-tauri/tauri.conf.json 注入 */
declare const __APP_VERSION__: string

/** 展示名（tauri.conf.json 的 productName 为小写包名，不适合直接展示） */
export const APP_NAME = 'Rhost'

/** 语义版本号，与安装包 / Cargo.toml / frontend/package.json 同源 */
export const APP_VERSION = __APP_VERSION__
