<script lang="ts">
  // A custom pet skin: one image per state, either a horizontal sprite sheet
  // (`frames` > 1) or an animated gif/webp/apng (`frames` omitted).
  import type { PetSkin } from "../lib/types";

  let { skin, state, level = 0 }: { skin: PetSkin; state: string; level?: number } = $props();

  // Missing states fall back to idle.
  const anim = $derived(skin.states[state] ?? skin.states["idle"]);
  const frames = $derived(Math.max(1, anim?.frames ?? 1));
  const duration = $derived(frames / Math.max(1, anim?.fps ?? 8));
  const pulse = $derived(anim?.micLevel ? 1 + Math.min(level * 2.2, 0.22) : 1);
</script>

{#if anim}
  {#if frames > 1}
    {#key state}
      <div
        class="sheet"
        style="background-image: url({anim.src}); --frames: {frames}; animation-duration: {duration}s;
               animation-iteration-count: {anim.loop === false ? 1 : 'infinite'}; transform: scale({pulse})"
      ></div>
    {/key}
  {:else}
    <img src={anim.src} alt="" style="transform: scale({pulse})" draggable="false" />
  {/if}
{/if}

<style>
  .sheet,
  img {
    width: 100%;
    height: 100%;
    image-rendering: pixelated;
    transition: transform 70ms linear;
  }
  img {
    object-fit: contain;
  }
  .sheet {
    background-repeat: no-repeat;
    /* The sheet is `frames` cells wide; step through them one cell at a time. */
    background-size: calc(var(--frames) * 100%) 100%;
    animation-name: play;
    animation-timing-function: steps(var(--frames), jump-none);
    animation-fill-mode: forwards;
  }
  @keyframes play {
    from {
      background-position-x: 0%;
    }
    to {
      background-position-x: 100%;
    }
  }
</style>
