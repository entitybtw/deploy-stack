FROM eclipse-temurin:21-jdk-alpine AS build
WORKDIR /src
COPY . .
RUN if [ -f "pom.xml" ]; then ./mvnw package -DskipTests; elif [ -f "build.gradle" ]; then ./gradlew bootJar; fi

FROM eclipse-temurin:21-jre-alpine
COPY --from=build /src/target/*.jar /app.jar
EXPOSE 8080
CMD ["java", "-jar", "/app.jar"]
