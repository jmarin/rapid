# RAPID
Robust Asynchronous Processing for Image Data

 
## Description

RAPID is a web service that allows a user to upload an image to a server; upon upload, several concurrent operations will take place on such an image including optimizations, transformations and analysis. Clients should be notified of progress on all operations. The number of derived products from the initial image will be fixed for this project, and those resources will also be downloadable on their respective paths.

### Main features

* Allow upload of image files through a web browser
* Every file will have its mime type checked upon upload, rejecting files that are not images
* Backend is an HTTP web service following typical REST patterns
* There is no authentication for this service at this point
* The system should allow uploading large files (several GB). This project will explore implementing resumable uploads for this
* When creating a new resource (image upload), the system should offer a way to track status and progress of the different image transformations
* The initial image transformations targeted for this project are
    - Thumbnail and preview generation in 3 sizes: 80x80, 250x250 and 800x600 pixels
    - High resolution: if the original file is larger than 3000px, a high resolution preview is generated (6000x6000 pixels)
* All images including original are stored in AWS S3 compatible storage and can be retrieved and downloaded from a client
* Metadata for images (minimal set to start with) will be stored in a Postgres database

### Technology stack



### Project structure


