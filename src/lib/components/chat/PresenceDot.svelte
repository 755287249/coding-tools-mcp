<script lang="ts">
  import type { sessionPresence } from '$lib/chat/navigation';
  let { state, label, group=false }: {state:ReturnType<typeof sessionPresence>;label:string;group?:boolean}=$props();
</script>
<span class="presence-dot" class:group data-state={state} role="img" aria-label={label} title={label}></span>
<style>
.presence-dot{display:inline-block;width:7px;height:7px;flex:none;border-radius:50%;background:#8a8f99;border:1px solid transparent}.group{border-radius:2px}
[data-state=working]{background:#479aff;animation:working-pulse 1.5s ease-in-out infinite;box-shadow:0 0 5px #479aff66}
[data-state=online]{background:#39b879;box-shadow:0 0 7px #39b87999}
[data-state=queued]{background:#d3a644;box-shadow:0 0 4px #d3a64444}
[data-state=awaiting_user]{background:#ad87ed;box-shadow:0 0 6px #ad87ed88}
[data-state=error]{background:#e07868;box-shadow:0 0 4px #e0786855}
[data-state=closed]{background:transparent;border-color:#8a8f99;opacity:.65}
@keyframes working-pulse{0%,100%{transform:scale(.85);opacity:.7}50%{transform:scale(1.25);opacity:1}}
@media(prefers-reduced-motion:reduce){[data-state=working]{animation:none}}
</style>
