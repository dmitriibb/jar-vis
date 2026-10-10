<script lang="ts">
  // The built-in pet: a round blob drawn in SVG and animated with CSS, so the
  // app ships without art assets.
  let { state, level = 0 }: { state: string; level?: number } = $props();

  // Mic level (RMS, usually 0–0.3) mapped to a visible pulse.
  const pulse = $derived(state === "listening" ? 1 + Math.min(level * 2.2, 0.22) : 1);
</script>

<svg class="blob {state}" viewBox="0 0 128 128" aria-hidden="true">
  <defs>
    <radialGradient id="body" cx="38%" cy="30%" r="80%">
      <stop offset="0%" stop-color="var(--hi)" />
      <stop offset="100%" stop-color="var(--lo)" />
    </radialGradient>
  </defs>

  {#if state === "listening"}
    <circle class="ring" cx="64" cy="68" r="50" />
  {/if}

  <g class="bounce">
    <g class="body" style="transform: scale({pulse})">
      <ellipse cx="64" cy="70" rx="44" ry="40" fill="url(#body)" />
      <ellipse class="shine" cx="48" cy="50" rx="12" ry="7" />

      {#if state === "sleeping"}
        <path class="lid" d="M42 68 q8 6 16 0" />
        <path class="lid" d="M70 68 q8 6 16 0" />
      {:else if state === "error"}
        <path class="lid" d="M43 60 l12 12 m0 -12 l-12 12" />
        <path class="lid" d="M73 60 l12 12 m0 -12 l-12 12" />
      {:else if state === "offline"}
        <path class="lid" d="M43 67 h13" />
        <path class="lid" d="M72 67 h13" />
      {:else}
        <g class="eyes">
          <ellipse cx="49" cy="66" rx="6" ry="8" />
          <ellipse cx="79" cy="66" rx="6" ry="8" />
        </g>
      {/if}

      {#if state === "speaking"}
        <ellipse class="mouth talk" cx="64" cy="88" rx="8" ry="5" />
      {:else if state === "error" || state === "offline"}
        <path class="lid" d="M54 92 q10 -7 20 0" />
      {:else if state === "sleeping"}
        <ellipse class="mouth" cx="64" cy="89" rx="3" ry="2.5" />
      {:else}
        <path class="lid" d="M54 86 q10 8 20 0" />
      {/if}
    </g>
  </g>

  {#if state === "transcribing" || state === "working" || state === "sending"}
    <g class="dots">
      <circle cx="46" cy="18" r="4" />
      <circle cx="64" cy="14" r="4" />
      <circle cx="82" cy="18" r="4" />
    </g>
  {/if}
  {#if state === "permission"}
    <g class="alert">
      <circle cx="104" cy="28" r="14" />
      <path d="M104 19 v11 M104 35 v1" />
    </g>
  {/if}
  {#if state === "sleeping"}
    <text class="zzz" x="92" y="34">z</text>
    <text class="zzz late" x="104" y="20">z</text>
  {/if}
</svg>

<style>
  .blob {
    --hi: #8ff5e3;
    --lo: #2b9d8d;
    width: 100%;
    height: 100%;
    overflow: visible;
    filter: drop-shadow(0 4px 6px rgba(0, 0, 0, 0.45));
  }
  .blob.error {
    --hi: #fca5a5;
    --lo: #b03a3a;
  }
  .blob.offline {
    --hi: #9aa3b5;
    --lo: #4a5265;
  }
  .blob.permission {
    --hi: #fde68a;
    --lo: #c98a16;
  }
  .blob.sleeping {
    --hi: #7fc9bd;
    --lo: #2a7269;
  }

  .body {
    transform-origin: 64px 76px;
    transition: transform 70ms linear;
  }
  .bounce {
    transform-origin: 64px 110px;
    animation: breathe 4s ease-in-out infinite;
  }
  .working .bounce,
  .sending .bounce {
    animation: bounce 0.7s ease-in-out infinite;
  }
  .permission .bounce {
    animation: wiggle 0.5s ease-in-out infinite;
  }
  .sleeping .bounce {
    animation: breathe 6s ease-in-out infinite;
  }
  .offline .bounce,
  .error .bounce {
    animation: none;
  }

  .shine {
    fill: rgba(255, 255, 255, 0.35);
  }
  .eyes ellipse,
  .mouth {
    fill: #10231f;
  }
  .eyes {
    transform-origin: 64px 66px;
    animation: blink 5s infinite;
  }
  .transcribing .eyes {
    animation: look 1.6s ease-in-out infinite;
  }
  .lid {
    fill: none;
    stroke: #10231f;
    stroke-width: 4;
    stroke-linecap: round;
  }
  .mouth.talk {
    transform-origin: 64px 88px;
    animation: talk 0.28s ease-in-out infinite alternate;
  }

  .ring {
    fill: none;
    stroke: var(--accent);
    stroke-width: 3;
    opacity: 0.7;
    transform-origin: 64px 68px;
    animation: ring 1.1s ease-out infinite;
  }
  .dots circle {
    fill: var(--accent);
    animation: dot 1s ease-in-out infinite;
  }
  .dots circle:nth-child(2) {
    animation-delay: 0.15s;
  }
  .dots circle:nth-child(3) {
    animation-delay: 0.3s;
  }
  .alert circle {
    fill: #dc2626;
  }
  .alert path {
    stroke: #fff;
    stroke-width: 4;
    stroke-linecap: round;
  }
  .alert {
    transform-origin: 104px 28px;
    animation: pop 0.8s ease-in-out infinite;
  }
  .zzz {
    fill: #cfe9e4;
    font: 700 16px sans-serif;
    animation: float 2.4s ease-in-out infinite;
  }
  .zzz.late {
    font-size: 12px;
    animation-delay: 1.2s;
  }

  @keyframes breathe {
    50% {
      transform: scale(1.03, 0.97);
    }
  }
  @keyframes bounce {
    50% {
      transform: translateY(-7px) scale(0.97, 1.04);
    }
  }
  @keyframes wiggle {
    25% {
      transform: rotate(-4deg);
    }
    75% {
      transform: rotate(4deg);
    }
  }
  @keyframes blink {
    0%,
    94%,
    100% {
      transform: scaleY(1);
    }
    97% {
      transform: scaleY(0.1);
    }
  }
  @keyframes look {
    25% {
      transform: translate(-3px, -2px);
    }
    75% {
      transform: translate(3px, -2px);
    }
  }
  @keyframes talk {
    from {
      transform: scaleY(0.35);
    }
    to {
      transform: scaleY(1.25);
    }
  }
  @keyframes ring {
    from {
      transform: scale(0.9);
      opacity: 0.8;
    }
    to {
      transform: scale(1.22);
      opacity: 0;
    }
  }
  @keyframes dot {
    0%,
    100% {
      transform: translateY(0);
      opacity: 0.4;
    }
    50% {
      transform: translateY(-5px);
      opacity: 1;
    }
  }
  @keyframes pop {
    50% {
      transform: scale(1.15);
    }
  }
  @keyframes float {
    0% {
      transform: translateY(4px);
      opacity: 0;
    }
    50% {
      opacity: 1;
    }
    100% {
      transform: translateY(-8px);
      opacity: 0;
    }
  }
</style>
