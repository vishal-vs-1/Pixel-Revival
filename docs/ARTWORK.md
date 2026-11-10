# Artwork comparisons

These Java ME examples show the effect of two optional artwork styles on saved assets.

## Real-game artwork: Mr Bean Racer 2

This is the same 176 × 220 title image from a local MrBeanRacer2 import, shown at 4× (704 × 880). The original uses nearest-neighbor enlargement; the other panels are the saved Fast Pixel and Real-ESRGAN AI Cartoon outputs from this project. This is extracted title artwork, not a gameplay frame.

<table>
  <thead>
    <tr>
      <th>Original · nearest-neighbor 4×</th>
      <th>Fast Pixel · 4×</th>
      <th>AI Cartoon · 4×</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td><img src="images/mrbean-racer-2-original-nearest-4x.png" alt="Mr Bean Racer 2 title image enlarged 4× with nearest-neighbor sampling" width="220"></td>
      <td><img src="images/mrbean-racer-2-fast-pixel-4x.png" alt="Same Mr Bean Racer 2 image enlarged 4× with Fast Pixel" width="220"></td>
      <td><img src="images/mrbean-racer-2-ai-cartoon-4x.png" alt="Same Mr Bean Racer 2 image enlarged 4× with Real-ESRGAN AI Cartoon" width="220"></td>
    </tr>
  </tbody>
</table>

<p><em>The MrBeanRacer2 JAR is not bundled; this comparison includes its title artwork. AI redraws the face and lettering, so the larger image is not a fully faithful restoration.</em></p>

## Bounce Tales artwork

This is the same 168 × 121 logo image from a local Bounce Tales import, shown at 4× (672 × 484). The first panel enlarges the original with nearest-neighbor sampling; the others show Fast Pixel and the locally generated Real-ESRGAN AI Cartoon output.

<table>
  <thead>
    <tr>
      <th>Original · nearest-neighbor 4×</th>
      <th>Fast Pixel · 4×</th>
      <th>AI Cartoon · 4×</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td><img src="images/bounce-tales-original-nearest-4x.png" alt="Bounce Tales logo enlarged 4× with nearest-neighbor sampling" width="220"></td>
      <td><img src="images/bounce-tales-fast-pixel-4x.png" alt="Bounce Tales logo enlarged 4× with Fast Pixel" width="220"></td>
      <td><img src="images/bounce-tales-ai-cartoon-4x.png" alt="Bounce Tales logo enlarged 4× with Real-ESRGAN AI Cartoon" width="220"></td>
    </tr>
  </tbody>
</table>

<p><em>The Bounce Tales JAR is not bundled; the images show one extracted logo resource, not gameplay. AI visibly redraws the ball and lettering, so inspect details before choosing this style for a game.</em></p>
