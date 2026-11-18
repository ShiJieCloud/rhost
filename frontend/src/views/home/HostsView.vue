<script setup lang="ts">
import HostToolbar from '../../components/HostToolbar.vue'
import HostList from '../../components/HostList.vue'
import { hosts } from '../../stores/hosts'
</script>

<template>
  <HostToolbar />

  <div class="divider"></div>

  <div class="list-meta">
    共 <b>{{ hosts.length }}</b> 台主机
  </div>

  <HostList />
</template>
