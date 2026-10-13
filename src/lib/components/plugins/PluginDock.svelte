<script lang="ts">
  import { onMount } from 'svelte';
  import { pluginPackages, installedAppPlugins, refreshPlugins } from '$lib/plugins/registry';
  import PluginFrame from './PluginFrame.svelte';
  onMount(()=>{void refreshPlugins()});
</script>
<div class="plugin-dock">{#each $pluginPackages.filter(item=>$installedAppPlugins.includes(item.manifest.id)) as item (item.manifest.id)}<PluginFrame package={item}/>{/each}</div>
<style>.plugin-dock{display:contents}</style>
