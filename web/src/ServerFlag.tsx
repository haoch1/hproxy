import { useEffect, useState } from "react";

let nativeFlags: boolean | undefined;
function supportsFlagEmoji() {
  if (nativeFlags !== undefined) return nativeFlags;
  const canvas = document.createElement("canvas");
  canvas.width = 48; canvas.height = 48;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return (nativeFlags = false);
  context.font = '32px "Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji", sans-serif';
  context.fillText("🇭🇰", 0, 36);
  const pixels = context.getImageData(0, 0, 48, 48).data;
  // Unsupported regional indicators render as monochrome letters, rather than a flag.
  for (let i = 0; i < pixels.length; i += 4) {
    if (pixels[i + 3] > 50 && Math.max(pixels[i], pixels[i + 1], pixels[i + 2]) - Math.min(pixels[i], pixels[i + 1], pixels[i + 2]) > 30)
      return (nativeFlags = true);
  }
  return (nativeFlags = false);
}

export default function ServerFlag({ country }: { country?: string }) {
  const code = country?.toUpperCase() || "";
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [code]);
  const valid = /^[A-Z]{2}$/.test(code) && !["XX", "PC"].includes(code);
  const name = valid
    ? new Intl.DisplayNames(["zh-CN"], { type: "region" }).of(code) || code
    : "地区未知";
  const emoji = valid ? String.fromCodePoint(...Array.from(code, (char) => 0x1f1e6 + char.charCodeAt(0) - 65)) : "🌐";
  return (
    <span className="server-region" data-country={code} title={name}>
      {valid && !failed ? (
        supportsFlagEmoji() ? <span className="flag-emoji" role="img" aria-label={name}>{emoji}</span> : <img
          src={`/flag-emoji/${code.toLowerCase()}.svg`}
          alt={name}
          width="28" height="28"
          draggable={false}
          onError={() => setFailed(true)}
        />
      ) : (
        <svg
          className="region-placeholder"
          width="28"
          height="21"
          viewBox="0 0 28 21"
          role="img"
          aria-label="地区待识别"
        >
          <rect width="28" height="21" rx="3" fill="#cce4f5" />
          <circle cx="14" cy="10.5" r="8" fill="#5299d1" />
          <path
            d="m9.1 4 4.5-.9 1.7 2.7-1 2.4-2.8.1-.8 3.1-3-1.7-1.2-2.1A8 8 0 0 1 9.1 4Zm8.4 4 3.6.7.8 3.1-2.4 1.5-1 3.2-2.6.9-.8-3-2.3-1.2.2-2.9 2.8-.5Z"
            fill="#b7ddc8"
          />
          <circle
            cx="14"
            cy="10.5"
            r="8"
            fill="none"
            stroke="#4488bd"
            strokeWidth=".6"
          />
        </svg>
      )}
    </span>
  );
}
