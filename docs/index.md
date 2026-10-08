---
layout: home

hero:
  name: Rhost
  text: 跨平台 SSH 远程主机管理器
  image:
    src: /logo.svg
    alt: Rhost
  tagline: Tauri 2 · Rust · Vue 3 —— 多标签终端、SFTP 文件管理、服务器监控
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
    details: xterm.js 渲染、UTF-8 全链路透传；高频字节流走二进制帧 Channel，断线指数退避自动重连。
  - icon: 📁
    title: 内置 SFTP 文件管理
    details: 双栏浏览、流式传输队列、断点续传、暂停/取消；Shell 与 SFTP 工作目录双向同步。
  - icon: 🔑
    title: 主机与密钥管理
    details: 连接配置本地持久化与原子写入；密码与私钥口令仅存系统钥匙串，绝不入配置文件。
  - icon: 📊
    title: 服务器监控
    details: MOTD 欢迎面板与 CPU、内存、磁盘、网络、进程、GPU 周期指标，经独立 exec 通道采集。
  - icon: ⚡
    title: 高性能纯异步内核
    details: russh 纯 async 实现，4KB/5ms 攒包、有界背压、单飞与错峰调度，100+ 会话不雪崩。
  - icon: 🛡️
    title: 安全与最小权限
    details: Tauri capabilities 白名单最小化；远端采集零命令拼接、零落盘、零 sudo、零常驻。
---
