<script lang="ts">
  import {memberTint} from '$lib/chat/collaboration';
  let {text,memberId}:{text:string;memberId:string}=$props();
</script>
<span class="robot-mention" style:--member-color={memberTint(memberId)}>{text}</span>
<style>
.robot-mention{padding:2px 0;border-radius:5px;background:color-mix(in srgb,var(--member-color) 7%,var(--card-bg));box-shadow:inset 0 0 0 1px color-mix(in srgb,var(--member-color) 30%,var(--color-border));color:inherit;box-decoration-break:clone;-webkit-box-decoration-break:clone}
</style>
