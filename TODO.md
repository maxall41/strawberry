## Alpha 0.1:
- [X] Decomposer tool that takes in one image and splits out each color channel as it's own image
- [X] Allow user to type values directly into value control + psuedo-slider
- [X] Saving and loading pipelines
- [X] CLI (Indiviudal, and directory processing supports regex for file filtering)
- [X] Support dragging and dropping images into viewer
- [X] Background subtraction with rolling ball
- [ ] GUI Error handling with toasts
- [X] Sobel filter
- [X] Grayscale
- [ ] Fallback to CPU for all image processors
- [ ] If compositing > 7 images then just batch into 7 image batches and run each and then composite them together (supports up to 49 images which should be enough for pretty much everything)
- [X] Safe indexing
- [ ] Auto updater
- [ ] Documentation

- [ ] If tokio backend panics handle it properly
- [ ] Refuses to load weka probability map file (the one that is 100+MB) with error: Failed to process pipeline after loading images: No input images provided
- [X] Make it so compositor is just a normal tool that can be disabled and enabled just like any other tool.
- [ ] Debounce pipeline events so if the user is changing the state rapdily like with a value counter it is all processed in a bundle
- [ ] Add debouncing to frontend value counter
- [ ] Don't allow user to enable compositor if there is only one image to prevent RGBA-BRGA mixups
- [ ] Fix Clippy Warnings

## Misc

- [ ] Abstract drop menu in tools/manager.rs into GUI component
- [ ] Run Lutz on GPU
- [ ] Instead of using PipelineCommand for all channels break it up so only nessecary commands need to be handled in match
- [ ] Cleanup BackendTool trait so tool manager does not need to have a bunch of stuff it doesn't need
- [ ] Clippy

## Other Features

- [ ] Command pallete
- [ ] Support Z-stacking
- [ ] Try replacing Flume with Kanal and see if it improves performance
- [ ] Support stitching
- [ ] Themes
- [X] Thresholding
- [X] Watershed segmentation
- [X] Local maxima detection/segmentation
- [ ] Weka segmentation with weka-rs
- [X] Manual/Autocell counter
- [ ] Flourescene intensity measurment tool
- [ ] Colocalization analysis tool
- [X] Morphological operations (Binary dilation, erosion, opening, closing)
- [ ] Add mode toggle to "Threshold" that in addition to manual thresholding supports otsu thresholding
- [ ] Settings menu with option to force CPU processing
- [ ] Multiple tool instances
- [ ] Make it so swithcing between auto and manual does not overwrite manual counts

## Big features:
- [ ] Parallel node based editor instead of linear flow
- [ ] Scripting with Rhai: https://github.com/rhaiscript/rhai
- [ ] Plugins
- [ ] Backend on a server

## Known issues:

This does not happen in release mode, nothing bad seems to happen so it is probably fine... : Crashes when trying to load CH2_weka_prepped.tif: assertion failed: self.table.is_full() ||
        (self.code_buffer.code_size() == 2 && self.next_code == 4) ||
    self.code_buffer.max_code() - Code::from(self.is_tiff) >= self.next_code
