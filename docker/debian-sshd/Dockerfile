# Rhost MOTD 全功能测试容器（Debian 版，替代 BusyBox 缺 lastlog 的 linuxserver/openssh-server）
# 构建:  docker build -t rhost-sshd-debian docker/debian-sshd
# 运行:  docker run -d --name rhost-test-sshd-debian -p 2223:22 rhost-sshd-debian
# 登录:  test / rhost123
FROM debian:bookworm-slim

# openssh-server: sshd 本体（postinst 自动生成 host key）
# iproute2: ip route（MOTD ip 采集）；procps: uptime/free/ps
# login 包（base 自带）: lastlog 命令 + pam_lastlog.so
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-server iproute2 procps \
 && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/bash test \
 && echo 'test:rhost123' | chpasswd \
 && mkdir -p /run/sshd

# 确保 SSH 登录经 PAM 写入 /var/log/lastlog（MOTD last_login 数据源）
RUN grep -qE '^session[^#]*pam_lastlog' /etc/pam.d/sshd \
 || echo 'session optional pam_lastlog.so' >> /etc/pam.d/sshd

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
