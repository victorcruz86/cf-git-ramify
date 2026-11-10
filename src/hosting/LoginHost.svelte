<script lang="ts">
  import { loginRequest } from "./accounts";
  import LoginDialog from "./LoginDialog.svelte";
</script>

{#if $loginRequest}
  {@const request = $loginRequest}
  <LoginDialog
    server={request.server}
    editing={request.editing}
    add={request.add}
    ondone={request.finish}
  />
{/if}
