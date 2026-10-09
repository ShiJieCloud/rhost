---
layout: home

hero:
  name: Rhost
  text: 跨平台 SSH 远程主机管理器
  image:
    src: /logo.svg
    alt: Rhost
  tagline: Tauri 2 · Rust · Vue 3 —— 多标签终端、SFTP 文件管理、端口转发、服务器监控，原生二进制流架构
  actions:
    - theme: brand
      text: 快速开始
      link: /development
    - theme: alt
      text: 架构概览
      link: /architecture

features:
  - icon: 🖥️
    title: 多标签 SSH 终端
    details: xterm.js 渲染、UTF-8 全链路透传；每会话独立 IPC Channel 数据流隔离，高频字节流走二进制帧，不经 JSON / Base64。
  - icon: ♻️
    title: 断线自动重连
    details: 区分断线原因，网络意外中断触发指数退避自动重连（1s 起翻倍、封顶 30s）；手动断开与远端正常退出不重连。
  - icon: 📁
    title: 内置 SFTP 文件管理
    details: 双栏浏览、流式传输队列、断点续传、暂停 / 取消；Shell 与 SFTP 工作目录经 OSC 6667 双向同步。
  - icon: 🔗
    title: SSH 端口转发
    details: 本地端口转发、远程端口转发、SOCKS5 动态代理；状态实时推送，连接数与流量 1s 节流合并。
  - icon: 📊
    title: 服务器监控
    details: MOTD 欢迎面板 + CPU / 内存 / 磁盘 / 网络 / 进程 / GPU 周期指标，经独立 exec 通道采集，零 sudo、零远端落盘。
  - icon: 🔐
    title: 主机密钥校验
    details: TOFU 首次信任 + known_hosts 指纹持久化，密钥变更弹红色警告，防中间人攻击。
  - icon: 🔑
    title: 安全凭证存储
    details: 密码与私钥口令仅存系统钥匙串（macOS 钥匙串 / Windows 凭据管理 / Linux libsecret），绝不入配置文件。
  - icon: 📤
    title: 配置导入导出
    details: 整体配置与主机列表批量导入导出，配置写操作串行化，导入后一键重启生效。
  - icon: 📝
    title: 应用日志系统
    details: 启动 / 退出埋点序列、日志持久化与过期清理、前端日志面板实时订阅与排查。
  - icon: ⚡
    title: 高性能纯异步内核
    details: russh 纯 async 实现，4KB/5ms 攒包、有界背压、单飞与错峰调度，多会话不雪崩、UI 永不阻塞。
  - icon: 🎨
    title: 跨平台原生桌面
    details: 基于 Tauri 2，体积小、内存占用低；无边框自绘标题栏、Splash 启动动画，支持 macOS / Windows / Linux。
---

## 界面预览

<div class="screenshot-main">

![工作台全景](assets/workbench-main.png)

工作台全景：多标签终端 + MOTD 欢迎面板 + SFTP 文件管理 + 实时服务器监控

</div>

<div class="screenshot-row">

<div class="shot">

![SFTP 文件管理](assets/sftp.png)

SFTP 双栏文件管理与传输队列

</div>

<div class="shot">

![端口转发](assets/tunnel-pane.png)

端口转发配置与命令预览

</div>

<div class="shot">

![快速连接](assets/quick-connect.png)

SSH 命令快速连接

</div>

</div>
