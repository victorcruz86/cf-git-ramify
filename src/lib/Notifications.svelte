<script lang="ts">
  import Icon from "./Icon.svelte";
  import { dismiss, notifications } from "./notifications";
</script>

<section class="notifications" aria-label="Notifications">
  {#each $notifications as notification (notification.id)}
    <div class="notification {notification.kind}" role={notification.kind === "error" ? "alert" : "status"}>
      <Icon name={notification.kind} />
      <div class="text">
        <div class="title">{notification.title}</div>
        {#if notification.body}
          <div class="body">{notification.body}</div>
        {/if}
        {#if notification.actions?.length}
          <div class="actions">
            {#each notification.actions as action}
              <button
                type="button"
                class="link"
                onclick={() => {
                  dismiss(notification.id);
                  action.run();
                }}>{action.label}</button
              >
            {/each}
          </div>
        {/if}
      </div>
      <button
        type="button"
        class="close"
        aria-label="Close"
        title="Close"
        onclick={() => dismiss(notification.id)}
      >
        <Icon name="close" />
      </button>
    </div>
  {/each}
</section>

<style>
  .notifications {
    position: fixed;
    right: 16px;
    bottom: 36px;
    z-index: 100;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 360px;
    max-width: calc(100vw - 32px);
    pointer-events: none;
  }

  .notification {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 10px 8px 10px 12px;
    color: var(--notification-text);
    pointer-events: auto;
    background: var(--notification-background);
    border: 1px solid var(--notification-border);
    border-radius: 8px;
    box-shadow: 0 4px 16px #00000066;
  }

  .notification :global(.icon) {
    margin-top: 1px;
  }

  .notification.error :global(.icon) {
    color: var(--error);
  }

  .notification.warning :global(.icon) {
    color: var(--editor-warning);
  }

  .text {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .title {
    font-weight: 600;
  }

  .body {
    margin-top: 2px;
    color: var(--text-secondary);
    white-space: pre-wrap;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    margin-top: 6px;
  }

  /* Not `.action`: in controls.css that is the toolbar's 26×26 icon button. */
  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    text-align: left;
    cursor: pointer;
    background: none;
    border: 0;
  }

  .link:hover {
    text-decoration: underline;
  }

  .close {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    background: none;
    border: 0;
    border-radius: 4px;
  }

  .close:hover {
    background: var(--hover);
  }
</style>
