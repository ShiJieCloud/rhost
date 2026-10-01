<script setup lang="ts">
import SideBar from '../components/SideBar.vue'
import HostToolbar from '../components/HostToolbar.vue'
import HostList from '../components/HostList.vue'
import { filterInfo, hosts, styleLabel } from '../stores/hosts'
</script>

<template>
  <div class="body">
    <SideBar />

    <main class="main">
      <HostToolbar />

      <div class="divider"></div>

      <div class="list-meta">
        共 <b>{{ hosts.length }}</b> 台主机
        <span class="sep">·</span>
        <span>当前显示：</span>
        <b>{{ styleLabel }}</b>
        <span class="sep">·</span>
        <span>{{ filterInfo }}</span>
      </div>

      <HostList />
    </main>
  </div>
</template>
