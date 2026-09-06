FROM mcr.microsoft.com/dotnet/sdk:8.0-alpine AS build
WORKDIR /src
COPY . .
RUN dotnet publish -c Release -o /app

FROM mcr.microsoft.com/dotnet/aspnet:8.0-alpine
COPY --from=build /app /app
EXPOSE 8080
WORKDIR /app
CMD ["dotnet", "app.dll"]
