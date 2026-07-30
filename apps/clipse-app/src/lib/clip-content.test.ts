import { describe, expect, it } from "vitest";

import { mimeForFormat, payloadDataUrl } from "./clip-content";

describe("mimeForFormat", () => {
  it("maps the three image formats", () => {
    expect(mimeForFormat("Png")).toBe("image/png");
    expect(mimeForFormat("Jpeg")).toBe("image/jpeg");
    expect(mimeForFormat("Svg")).toBe("image/svg+xml");
  });

  it("returns null for formats that are not images", () => {
    expect(mimeForFormat("Text")).toBeNull();
    expect(mimeForFormat("FileList")).toBeNull();
    expect(mimeForFormat({ Other: "text/csv" })).toBeNull();
  });
});

describe("payloadDataUrl", () => {
  it("builds a data URL from base64 the daemon returned", () => {
    expect(payloadDataUrl("Png", "aGk=")).toBe("data:image/png;base64,aGk=");
  });

  it("refuses to build one for a non-image format", () => {
    // A data: URL for something the panel will not render as an image is a
    // footgun waiting for the next person who points an <img> at it.
    expect(payloadDataUrl("Text", "aGk=")).toBeNull();
  });
});
